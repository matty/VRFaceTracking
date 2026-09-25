import json
import math
import os
from pathlib import Path
from contextlib import contextmanager
import shutil
import uuid
from unittest.mock import patch
import sys
import subprocess
import struct
import unittest

import numpy as np
import torch
from torch.nn import functional as F

from prepare_vrft_tongue import FRAME_BYTES, TARGET_NAMES, read_session, usable_samples
from train_vrft_tongue import (GATE, DIRECTION, Frames, best_runs, calibrate, calibrate_gate,
                               classify, coverage, disabled_names, loss_for, recording_paths,
                               regression_weights, supported_targets, trainable_targets,
                               visibility_loss, main, progress, write_json, augment_batch)
from tongue_inference import disabled_targets, inputs, load_checkpoint
from qpro_model import create_model

INFERENCE = Path(__file__).resolve().parent / 'tongue_inference.py'


@contextmanager
def test_directory():
    root = Path(__file__).resolve().parent / ".local" / "tongue-tests"
    directory = root / uuid.uuid4().hex
    directory.mkdir(parents=True)
    try:
        yield directory
    finally:
        assert directory.resolve().parent == root.resolve()
        shutil.rmtree(directory)


def recording(path, values=(0, 1), missing_native=False):
    path.mkdir()
    (path / "metadata.json").write_text(json.dumps(dict(format="vrft-tongue-capture-v1",
        bytesPerFrame=FRAME_BYTES, targets=TARGET_NAMES)))
    samples=[]
    with (path / "frames.gray8").open("wb") as raw:
        for index, value in enumerate(values):
            raw.write(bytes([value]) * FRAME_BYTES)
            samples.append(dict(index=index, step=index, pose=f"Pose {index}",
                targets=[float(value > 0)] + [0.] * 9,
                native_tongue_out=None if missing_native else float(value > 0)))
    (path / "samples.jsonl").write_text("".join(json.dumps(s) + "\n" for s in samples))
    return samples


def run_inference(gate, direction, frames):
    """Drive tongue_inference.py over its binary protocol, as the daemon does."""
    payload = b''.join(struct.pack('<Q', sequence) + frame.tobytes() for sequence, frame in frames)
    result = subprocess.run([sys.executable, str(INFERENCE), '--gate', str(gate), '--direction', str(direction)],
        input=payload, capture_output=True, timeout=60, check=True,
        env=dict(os.environ, VRFT_TONGUE_DEVICE='cpu'))
    handshake, packets = result.stdout.split(b'\n', 1)
    size = struct.calcsize('<Q10f')
    assert len(packets) == size * len(frames), result.stderr
    outputs = [struct.unpack_from('<Q10f', packets, i * size) for i in range(len(frames))]
    return json.loads(handshake), [(sequence, np.array(values)) for sequence, *values in outputs]


class Poses:
    """Minimal dataset stand-in: per-frame targets, native TongueOut and pose names."""
    def __init__(self, poses):
        self.targets = np.asarray([t for _, t, count in poses for _ in range(count)], np.float32)
        self.native = self.targets[:, 0].copy()
        self.pose_keys = [name for name, _, count in poses for _ in range(count)]

    def __len__(self):
        return len(self.targets)


def target(visible=1., extension=0., horizontal=0., vertical=0.):
    return [visible, extension, horizontal, vertical] + [0.] * 6


def base_models(temp):
    """Random starting checkpoints at a tiny image size."""
    base=temp/'base'; base.mkdir()
    model=create_model('legacy-late-fusion-v1',TARGET_NAMES)
    checkpoint=dict(architecture='legacy-late-fusion-v1',targetNames=TARGET_NAMES,
        imageSize=32,modelState=model.state_dict(),visibilityGate=dict(cameraWeight=.95,threshold=.85))
    for filename in (GATE,DIRECTION): torch.save(checkpoint,base/filename)
    return base


GRADED_STEPS={3:target(1.,.5,.5),4:target(1.,1.,-1.),5:target(1.,.75,0.,.5),
              6:target(1.,1.,.7,-.7),7:target(1.,.25)}


def graded_recording(temp,index):
    """64 frames in eight 8-frame poses: three hidden, five graded visible."""
    directory=temp/f'recording-{index}'
    samples=recording(directory,list(range(1+index*64,65+index*64)))
    for i,sample in enumerate(samples):
        step=i//8
        labels=GRADED_STEPS.get(step,target(0.))
        sample.update(step=step,pose=f'Pose {step}',targets=labels,native_tongue_out=labels[0])
    (directory/'samples.jsonl').write_text(''.join(json.dumps(s)+'\n' for s in samples))
    return str(directory)


def train(temp,base,recordings):
    """Runs the trainer for one pass; returns its output folder and report."""
    request=temp/'request.json'
    request.write_text(json.dumps(dict(recordings=recordings,base_model_dir=str(base),device='cpu',name='Synthetic regression check')))
    output=temp/'output'
    with patch.object(sys,'argv',['train','--request',str(request),'--output',str(output),'--epochs','1']): main()
    return output,json.loads((output/'report.json').read_text())


GRADED_POSES = [
    ("Neutral", target(0.), 12), ("Speech", target(0.), 12),
    ("Tip", target(1., .25), 8), ("Half out", target(1., .5), 8),
    ("Mostly out", target(1., .75), 8), ("Full out", target(1., 1.), 8),
    ("Left half", target(1., 1., -.5), 8), ("Right half", target(1., 1., .5), 8),
    ("Up", target(1., 1., 0., 1.), 8), ("Down half", target(1., 1., 0., -.5), 8),
    ("Up right", target(1., 1., .7, .7), 8), ("Down left", target(1., 1., -.7, -.7), 8),
]


class TrainingTests(unittest.TestCase):
    def test_locked_progress_file_is_retried_and_never_fatal(self):
        with test_directory() as temp:
            real_replace=os.replace; calls=[]
            def locked_twice(source,target):
                calls.append(target)
                if len(calls)<=2: raise PermissionError(5,"Access is denied")
                real_replace(source,target)
            with patch('train_vrft_tongue.os.replace',locked_twice), patch('train_vrft_tongue.time.sleep'):
                write_json(temp/'progress.json',{'stage':'training'})
            self.assertEqual(len(calls),3)
            self.assertEqual(json.loads((temp/'progress.json').read_text())['stage'],'training')
            def always_locked(source,target): raise PermissionError(5,"Access is denied")
            with patch('train_vrft_tongue.os.replace',always_locked), patch('train_vrft_tongue.time.sleep'):
                progress(temp,'training','Still going')
                with self.assertRaises(PermissionError): write_json(temp/'report.json',{})
            self.assertEqual(json.loads((temp/'progress.json').read_text())['stage'],'training')

    def test_skip_and_review_exclude_already_recorded_frames(self):
        with test_directory() as temp:
            directory=Path(temp) / "capture"; recording(directory, (0, 1, 2))
            (directory / "excluded_steps.json").write_text('{"excluded_steps":[0]}')
            (directory / "review.json").write_text('{"excluded_steps":[2]}')
            samples,_=usable_samples(directory)
            self.assertEqual([s["index"] for s in samples], [1])
            self.assertEqual(len(read_session(directory)[0]), 3)

    def test_missing_native_excluded_and_corrupt_targets_rejected(self):
        with test_directory() as temp:
            directory=Path(temp) / "capture"; samples=recording(directory, missing_native=True)
            self.assertEqual(usable_samples(directory)[0], [])
            samples[0]["targets"][0]=float("nan")
            (directory / "samples.jsonl").write_text("".join(json.dumps(s)+"\n" for s in samples))
            with self.assertRaisesRegex(ValueError,"Invalid targets"): read_session(directory)

    def test_every_selected_recording_is_used_once(self):
        with test_directory() as temp:
            a,b=Path(temp)/'a',Path(temp)/'b'
            paths=recording_paths(dict(recordings=[str(a),str(b),str(a)]))
            self.assertEqual(paths,[a.resolve(),b.resolve()])
            for request in ({},dict(recordings=[])):
                with self.assertRaisesRegex(ValueError,'at least one recording'):
                    recording_paths(request)

    def test_label_only_frames_skip_image_reads(self):
        with test_directory() as temp:
            directory=temp/'capture'; samples=recording(directory,range(1,17))
            for sample in samples: sample['step']=sample['index']//8
            (directory/'samples.jsonl').write_text(''.join(json.dumps(s)+'\n' for s in samples))
            labels=Frames([directory],32,images=False)
            self.assertEqual((len(labels),labels.arrays),(16,[]))
            self.assertEqual(len(Frames([directory],32)),16)

    def test_unsupported_detail_heads_have_no_regression_gradient(self):
        output=torch.full((2,10),.4,requires_grad=True)
        target=torch.tensor([[1.,1.,1.,0.,0.,0.,0.,0.,0.,0.],[0.]*10])
        loss_for(output,target,[True]*4+[False]*6,"direction").backward()
        self.assertEqual(float(output.grad[:,4:].abs().sum()),0.)
        self.assertGreater(float(output.grad[:,1:4].abs().sum()),0.)

    def test_visibility_loss_weights_hidden_frames_more(self):
        half=torch.full((1,10),.5)
        visible=visibility_loss(half,torch.tensor([target(1.)]))
        hidden=visibility_loss(half,torch.tensor([target(0.)]))
        self.assertAlmostEqual(float(visible),1.25*math.log(2),places=5)
        self.assertAlmostEqual(float(hidden),1.70*math.log(2),places=5)
        self.assertEqual(float(loss_for(half,torch.tensor([target(0.)]),[True]*10,"gate")),float(hidden))

    def test_hand_written_bce_matches_torch_and_survives_half_precision(self):
        probability=torch.rand(64,10)*.98+.01
        expected=torch.zeros(64,10); expected[::2,0]=1
        reference=F.binary_cross_entropy(probability[:,0],expected[:,0],reduction='none')
        weights=torch.where(expected[:,0]>=.5,1.25,1.70)
        self.assertAlmostEqual(float(visibility_loss(probability,expected)),
                               float((reference*weights).mean()),places=5)
        # float16 sigmoid saturates to exactly 0 or 1; the loss must stay finite.
        saturated=torch.tensor([[1.]+[0.]*9,[0.]*10],dtype=torch.float16)
        self.assertTrue(torch.isfinite(visibility_loss(saturated,torch.tensor([target(0.),target(1.)]))))

    def test_regression_weights_follow_columns_and_active_boost(self):
        labels=torch.tensor([[1.,.5,-.7,.7,0.,0.,0.,0.,0.,.05],target(0.,1.,1.)])
        weights=regression_weights(labels,[True]*10)
        expected=[0.,1.4*3,2.2*7,2.2*7,2.5,2.5,2.5,2.2,2.2,2.]
        np.testing.assert_allclose(weights[0].numpy(),expected,rtol=1e-6)
        self.assertEqual(float(weights[1].abs().sum()),0.)  # hidden frames never regress
        masked=regression_weights(labels,[True]*4+[False]*6)
        np.testing.assert_allclose(masked[0].numpy(),expected[:4]+[0.]*6,rtol=1e-6)
        prediction=torch.rand(2,10)*.8+.1
        error=F.smooth_l1_loss(prediction,labels,reduction='none',beta=.08)
        total=1.8*visibility_loss(prediction,labels)+(error*weights).sum()/weights.sum()
        self.assertAlmostEqual(float(loss_for(prediction,labels,[True]*10,"direction")),float(total),places=5)

    def test_signed_head_needs_both_directions(self):
        class Data: pass
        data=Data(); data.targets=np.zeros((60,10),np.float32)
        data.targets[20:,0]=1
        data.targets[20:40,2]=1
        self.assertFalse(supported_targets(data)[2])
        data.targets[40:,2]=-1
        mask=supported_targets(data)
        self.assertTrue(mask[2]); self.assertFalse(mask[4])

    def test_graded_labels_enable_core_heads_and_disable_unrecorded_shapes(self):
        data=Poses(GRADED_POSES)
        enabled=supported_targets(data)
        self.assertEqual(enabled,[True]*4+[False]*6)
        self.assertEqual(disabled_names(enabled),TARGET_NAMES[4:])
        report=coverage(data)
        self.assertEqual(report['targets']['extension']['levels'],[.25,.5,.75,1.])
        self.assertEqual(report['targets']['horizontal']['levels'],[-.7,-.5,.5,.7])
        self.assertEqual((report['targets']['vertical']['positive'],report['targets']['vertical']['negative']),(16,16))
        self.assertEqual(report['targets']['roll'],dict(positive=0,negative=0,levels=[]))
        self.assertEqual(trainable_targets(data),enabled)

    def test_missing_basic_directions_are_named(self):
        # Graded poses cover up only once and never cover right.
        data=Poses([pose for pose in GRADED_POSES if pose[0] not in ('Right half','Up right','Up')])
        with self.assertRaisesRegex(ValueError,'No usable tongue right, up poses'):
            trainable_targets(data)

    def test_training_frames_calibrate_threshold_at_the_preferred_weight(self):
        # Frames the model trained on separate perfectly; the blend weight
        # must not drift to camera-only, and native TongueOut still counts.
        data=Poses(GRADED_POSES)
        prediction=data.targets.copy()
        config=calibrate(prediction,data)
        self.assertEqual(config['cameraWeight'],.8)
        self.assertTrue(.3<=config['threshold']<=.8)

    def test_false_positive_reporting(self):
        metrics=classify(np.array([.9,.8,.1,.7]),np.array([1,0,0,1]),.5)
        self.assertEqual(metrics['fp'],1)
        self.assertEqual(metrics['false_positive_rate'],.5)
        self.assertEqual(metrics['false_negative_rate'],0.)
        self.assertEqual(metrics['recall'],1.)

    def test_best_runs_are_contiguous_plateaus(self):
        self.assertEqual(best_runs([0,1,1,0,1,1,1],1),[(1,2),(4,6)])
        self.assertEqual(best_runs([.5,.5],.5),[(0,1)])

    def test_gate_calibration_takes_widest_plateau_midpoint(self):
        # Camera separates at 0.205/0.895; native is perfect. Every threshold
        # in 0.11-0.94 is perfect at weight 0.5, so the old largest-tie rule
        # chose 0.94 -- the edge of the plateau.
        expected=np.array([0.]*24+[1.]*40)
        camera=np.where(expected>0,.895,.205)
        config=calibrate_gate(camera,expected,expected)
        self.assertEqual(config['cameraWeight'],.5)
        self.assertEqual(config['plateau'],[.11,.94])
        self.assertAlmostEqual(config['threshold'],.525)

    def test_gate_calibration_weight_tie_prefers_0_8(self):
        expected=np.array([0.]*24+[1.]*40)
        camera=np.where(expected>0,.695,.305)
        config=calibrate_gate(camera,camera,expected)  # blend weight cannot matter
        self.assertEqual(config['cameraWeight'],.8)
        self.assertEqual(config['plateau'],[.31,.69])
        self.assertAlmostEqual(config['threshold'],.5)

    def test_gate_calibration_threshold_is_clamped(self):
        expected=np.array([0.]*24+[1.]*40)
        high=calibrate_gate(np.where(expected>0,.97,.88),np.full(64,.5),expected)
        self.assertEqual((high['cameraWeight'],high['plateau'],high['threshold']),(.9,[.85,.92],.8))
        low=calibrate_gate(np.where(expected>0,.15,.02),np.zeros(64),expected)
        self.assertEqual((low['cameraWeight'],low['plateau'],low['threshold']),(1.,[.1,.15],.3))

    def test_disabled_target_metadata(self):
        self.assertEqual(disabled_targets({}),[])  # starting checkpoints keep all heads
        self.assertEqual(disabled_targets(dict(supportedTargets=[True]*4+[False]*6)),TARGET_NAMES[4:])
        self.assertEqual(disabled_targets(dict(disabledTargets=['twist','roll'],supportedTargets=[True]*10)),
                         ['roll','twist'])
        with self.assertRaisesRegex(ValueError,'visibility'):
            disabled_targets(dict(disabledTargets=['visibility']))
        with self.assertRaisesRegex(ValueError,'Unknown'):
            disabled_targets(dict(disabledTargets=['tongue_roll']))

    def test_inference_is_raw_per_frame_and_zeroes_disabled_heads(self):
        with test_directory() as temp:
            torch.manual_seed(7)
            paths={}
            for name,disabled in (('gate',['roll']),('direction',['horizontal','roll','twist'])):
                model=create_model('legacy-late-fusion-v1',TARGET_NAMES)
                for layer in model.modules():  # make random weights respond visibly to the image
                    if isinstance(layer,torch.nn.BatchNorm2d): layer.running_var.fill_(.05)
                paths[name]=temp/f'{name}.pt'
                torch.save(dict(architecture='legacy-late-fusion-v1',targetNames=TARGET_NAMES,imageSize=32,
                    modelState=model.state_dict(),visibilityGate=dict(cameraWeight=.8,threshold=.55),
                    disabledTargets=disabled),paths[name])
            flat=np.full((400,800),40,np.uint8)
            ramp=np.tile(np.linspace(0,255,800).astype(np.uint8),(400,1))
            handshake,outputs=run_inference(paths['gate'],paths['direction'],[(5,flat),(6,ramp),(7,flat)])
            self.assertEqual(handshake['version'],1)
            self.assertEqual(handshake['smoothing'],'none')
            self.assertEqual(handshake['disabled_targets'],['horizontal','roll','twist'])
            self.assertEqual((handshake['camera_weight'],handshake['threshold'],handshake['device']),(.8,.55,'cpu'))
            device=torch.device('cpu')
            gate,_,_=load_checkpoint(str(paths['gate']),device)
            direction,_,_=load_checkpoint(str(paths['direction']),device)
            expected=[]
            with torch.inference_mode():
                for frame in (flat,ramp):
                    values=direction(inputs(frame,32,device))[0].numpy().astype(np.float64)
                    values[0]=float(gate(inputs(frame,32,device))[0,0])
                    expected.append(values)
            self.assertGreater(np.abs(expected[0]-expected[1]).max(),1e-3)
            for (sequence,values),reference in zip(outputs,expected+expected[:1]):
                self.assertIn(sequence,(5,6,7))
                self.assertEqual([values[i] for i in (2,6,9)],[0.,0.,0.])
                keep=[i for i in range(10) if i not in (2,6,9)]
                # No temporal smoothing: each packet equals that frame's own prediction.
                np.testing.assert_allclose(values[keep],reference[keep],atol=1e-4)
            self.assertEqual([sequence for sequence,_ in outputs],[5,6,7])

    def test_follow_frames_skip_the_pose_cap_and_sample_by_direction(self):
        with test_directory() as temp:
            directory=temp/'capture'; directory.mkdir()
            (directory/'metadata.json').write_text(json.dumps(dict(format='vrft-tongue-capture-v1',
                bytesPerFrame=FRAME_BYTES,targets=TARGET_NAMES)))
            samples=[]
            def add(**fields): samples.append(dict(index=len(samples),native_tongue_out=0.,**fields))
            for i in range(95): add(step=0,pose='Tongue left',targets=target(1.,1.,-1.))
            for h,v in [(1.,0.),(-.35,.35),(.1,0.)]*32: add(step=1,pose='Follow the dot',dot=[h,v],targets=target(1.,1.,h,v))
            (directory/'samples.jsonl').write_text(''.join(json.dumps(s)+'\n' for s in samples))
            with (directory/'frames.gray8').open('wb') as raw: raw.truncate(len(samples)*FRAME_BYTES)
            frames=Frames([directory],32,images=False)
            keys=frames.pose_keys
            self.assertEqual(keys.count('Tongue left'),90)  # held poses keep the cap
            self.assertEqual(sum(key.startswith('Follow the dot') for key in keys),96)
            self.assertEqual({key for key in keys if key.startswith('Follow')},
                             {'Follow the dot: right','Follow the dot: half up left','Follow the dot: centre'})
            self.assertEqual(coverage(frames)['sources'],dict(follow=96,poses=90))

    def test_augmentation_moves_both_views_together(self):
        torch.manual_seed(3)
        # A fine checkerboard: any geometry difference between the views shows
        # up as large pixel differences, while sensor noise (independent per
        # view by design) stays tiny.
        board=((torch.arange(32)[:,None]//2+torch.arange(32)[None,:]//2)%2).float()
        images=board.expand(16,2,32,32).clone()
        changed=augment_batch(images)
        self.assertEqual(tuple(changed.shape),(16,2,32,32))
        self.assertTrue(float(changed.min())>=0 and float(changed.max())<=1)
        self.assertLess(float((changed[:,0]-changed[:,1]).abs().max()),.06)
        moved=(changed-images).abs().flatten(1).max(1).values
        self.assertGreater(int((moved>.2).sum()),12)  # nearly every frame changes

    def test_follow_recordings_train_end_to_end(self):
        with test_directory() as temp:
            base=base_models(temp)
            follow=temp/'follow'; samples=recording(follow,range(30,54))
            for i,sample in enumerate(samples):
                h,v=math.cos(i/4),math.sin(i/4)
                sample.update(step=0,pose='Follow the dot',dot=[h,v],targets=target(1.,1.,h,v),native_tongue_out=1.)
            (follow/'samples.jsonl').write_text(''.join(json.dumps(s)+'\n' for s in samples))
            output,report=train(temp,base,[graded_recording(temp,0),str(follow)])
            self.assertEqual(report['coverage']['sources'],dict(follow=24,poses=64))
            self.assertTrue(.3<=report['calibration']['threshold']<=.8)
            self.assertTrue((output/DIRECTION).is_file())

    def test_full_training_report_and_inference_contract(self):
        """Small independent synthetic recordings with graded labels exercise the real Torch path."""
        with test_directory() as temp:
            base=base_models(temp)
            recordings=[graded_recording(temp,index) for index in range(2)]
            output,report=train(temp,base,recordings)
            self.assertEqual((report['frames'],report['coverage']['frames'],report['epochs']),(128,128,1))
            self.assertEqual(report['recordings'],[str(Path(path).resolve()) for path in recordings])
            self.assertEqual(report['coverage']['targets']['horizontal']['levels'],[-1.,.5,.7])
            self.assertEqual(report['supported_targets'],TARGET_NAMES[:4])
            self.assertEqual(report['disabled_targets'],TARGET_NAMES[4:])
            self.assertEqual(report['calibration']['camera_weight'],.8)
            self.assertTrue(.3<=report['calibration']['threshold']<=.8)
            finished=json.loads((output/'progress.json').read_text())
            self.assertEqual((finished['stage'],finished['fraction']),('complete',1.))
            gate_config=None
            for filename in (GATE,DIRECTION):
                saved=torch.load(output/filename,map_location='cpu',weights_only=True)
                self.assertEqual(saved['disabledTargets'],TARGET_NAMES[4:])
                self.assertEqual(saved['supportedTargets'],[True]*4+[False]*6)
                self.assertEqual(saved['personalTraining']['epochs'],1)
                gate_config=gate_config or saved['visibilityGate']
                self.assertEqual(saved['visibilityGate'],gate_config)
            handshake,outputs=run_inference(output/GATE,output/DIRECTION,[(123,np.zeros((400,800),np.uint8))])
            self.assertEqual(handshake['version'],1)
            self.assertEqual(handshake['disabled_targets'],TARGET_NAMES[4:])
            self.assertEqual(handshake['threshold'],report['calibration']['threshold'])
            sequence,values=outputs[0]
            self.assertEqual(sequence,123)
            self.assertTrue(np.isfinite(values).all())
            self.assertEqual(list(values[4:]),[0.]*6)


if __name__ == '__main__': unittest.main()
