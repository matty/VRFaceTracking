// Step 2 of the preview: choose recordings, train, and pick the model in use.
// Uses byId, store, getJson, post, notice, icon, speak and the capture state from the page.
const RECORDING_KINDS = {core: 'Basic poses', direction: 'Direction poses', negatives: 'Expression poses',
  follow: 'Follow the dot'};
// What the trainer needs across the ticked recordings, as visible frames.
const NEEDS = [['out', 20, 'tongue out'], ['in', 20, 'tongue in'], ['left', 8, 'tongue left'],
  ['right', 8, 'tongue right'], ['up', 8, 'tongue up'], ['down', 8, 'tongue down']];
let recordings = [];
const unticked = new Set(store.get('untickedRecordings', []));
let savedModels = [];
let activeModelId = 'demo';
let modelOverride = false;
let shownJob = null;

function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}
const createdAt = id => Number(String(id).split('-')[0]);
// Mid-sentence, `lower` gives "today 12:30" rather than "Today 12:30".
function when(ms, lower = false) {
  const date = new Date(ms);
  if (!Number.isFinite(ms) || Number.isNaN(date.getTime())) return 'Recording';
  const time = date.toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});
  const today = new Date();
  const yesterday = new Date(today); yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return `${lower ? 'today' : 'Today'} ${time}`;
  if (date.toDateString() === yesterday.toDateString()) return `${lower ? 'yesterday' : 'Yesterday'} ${time}`;
  return `${date.toLocaleDateString([], {day: 'numeric', month: 'short'})} ${time}`;
}
function duration(seconds) {
  if (!Number.isFinite(seconds)) return '';
  if (seconds < 60) return 'under a minute';
  const minutes = Math.round(seconds / 60);
  return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}
const plural = (count, word) => `${count.toLocaleString()} ${word}${count === 1 ? '' : 's'}`;
const listed = items => items.length < 2 ? items.join('') : `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`;

function tickedRecordings() {
  return recordings.filter(r => !r.error && !unticked.has(r.id));
}
function missingPoses(list) {
  return NEEDS.filter(([key, needed]) => list.reduce((sum, r) => sum + (r.coverage?.[key] || 0), 0) < needed)
    .map(([, , name]) => name);
}

async function drawRecordedFrame(canvas, id, index) {
  const response = await fetch(`/ext/quest-pro/training/frame?id=${encodeURIComponent(id)}&index=${index}`, {cache: 'no-store'});
  if (!response.ok) throw new Error(await response.text());
  const gray = new Uint8Array(await response.arrayBuffer());
  if (gray.length !== 320000) throw new Error('Incomplete recorded frame');
  const context = canvas.getContext('2d');
  const image = context.createImageData(800, 400);
  for (let i = 0; i < gray.length; i++) {
    image.data[i * 4] = image.data[i * 4 + 1] = image.data[i * 4 + 2] = gray[i]; image.data[i * 4 + 3] = 255;
  }
  context.putImageData(image, 0, 0);
}

// Optional: look at each pose and leave out any that went wrong.
function poseReview(recording) {
  const grid = element('div', undefined, 'pose-review');
  // Follow the dot repeats its part names; number the repeats.
  const totals = {}, seen = {};
  for (const pose of recording.poses) totals[pose.name] = (totals[pose.name] || 0) + 1;
  for (const pose of recording.poses) {
    const card = element('div', undefined, 'pose-tile');
    const include = element('input'); include.type = 'checkbox';
    include.checked = !pose.excluded; include.disabled = pose.skipped;
    seen[pose.name] = (seen[pose.name] || 0) + 1;
    const name = totals[pose.name] > 1 ? `${pose.name} ${seen[pose.name]}` : pose.name;
    const label = element('label', `${name}${pose.skipped ? ' (skipped)' : ''}`);
    label.prepend(include);
    include.onchange = async () => {
      const previous = pose.excluded;
      pose.excluded = !include.checked; include.disabled = true;
      try {
        await post('/ext/quest-pro/training/review', {id: recording.id, excluded_steps: recording.poses.filter(p => p.excluded).map(p => p.step)});
      } catch (error) {
        pose.excluded = previous; include.checked = !previous;
        notice('recordings-message', error.message, 'bad');
      } finally { include.disabled = pose.skipped; }
    };
    const canvas = element('canvas'); canvas.width = 800; canvas.height = 400;
    canvas.setAttribute('aria-label', `${pose.name}: recorded camera frame`);
    const slider = element('input'); slider.type = 'range'; slider.min = 0; slider.max = 2; slider.value = 1;
    slider.setAttribute('aria-label', `${pose.name}: start, middle or end frame`);
    const draw = () => drawRecordedFrame(canvas, recording.id, pose.indices[Number(slider.value)])
      .catch(error => notice('recordings-message', error.message, 'bad'));
    slider.oninput = draw;
    card.append(label, canvas, slider); grid.append(card); draw();
  }
  return grid;
}

async function deleteRecording(recording) {
  const kind = RECORDING_KINDS[recording.mode] || 'unnamed';
  if (!confirm(`Delete the ${kind} recording from ${when(createdAt(recording.id), true)}? Its camera frames will be removed from this PC.`)) return;
  try {
    await post('/ext/quest-pro/training/delete', {id: recording.id});
    unticked.delete(recording.id); store.set('untickedRecordings', [...unticked]);
    await refreshRecordings();
  } catch (error) { notice('recordings-message', `Couldn't delete: ${error.message}`, 'bad'); }
}

function renderRecordings() {
  const container = byId('recordings');
  container.replaceChildren();
  if (!recordings.length) container.append(element('p', 'No recordings yet.', 'empty'));
  for (const recording of [...recordings].reverse()) {
    const box = element('div', undefined, 'recording');
    const row = element('div', undefined, 'recording-row');
    const label = element('label', undefined, 'recording-pick');
    const tick = element('input'); tick.type = 'checkbox';
    tick.checked = !recording.error && !unticked.has(recording.id); tick.disabled = !!recording.error;
    tick.setAttribute('aria-label', 'Train on this recording');
    tick.onchange = () => {
      if (tick.checked) unticked.delete(recording.id); else unticked.add(recording.id);
      store.set('untickedRecordings', [...unticked]);
      updateTrainButton();
    };
    const text = element('span');
    text.append(element('span', `${when(createdAt(recording.id))} · ${RECORDING_KINDS[recording.mode] || 'Recording'}`, 'recording-title'),
      element('span', recording.error ? recording.error : `${plural(recording.frames, 'frame')}`, 'recording-meta'));
    label.append(tick, text);
    const [tagText, tone] = recording.error ? ['Unreadable', 'bad'] : recording.basic_ready ? ['Complete', 'ok'] :
      recording.mode === 'core' ? ['Incomplete', 'warn'] : ['Extra', ''];
    const actions = element('div', undefined, 'recording-actions');
    if (!recording.error) {
      const check = element('button', 'Review', 'ghost small'); check.type = 'button';
      check.title = 'Look through each pose and leave out any that went wrong';
      let review = null;
      check.onclick = () => {
        if (!review) { review = poseReview(recording); box.append(review); } else review.hidden = !review.hidden;
        check.textContent = review.hidden ? 'Review' : 'Hide';
      };
      actions.append(check);
    }
    const remove = element('button', undefined, 'ghost small icon-only danger'); remove.type = 'button';
    remove.append(icon('trash'));
    remove.title = 'Delete recording'; remove.setAttribute('aria-label', 'Delete recording');
    remove.onclick = () => deleteRecording(recording);
    actions.append(remove);
    row.append(label, element('span', tagText, `tag ${tone}`), actions);
    box.append(row); container.append(box);
  }
  updateTrainButton();
}

async function refreshRecordings() {
  try {
    recordings = await getJson('/ext/quest-pro/training/sessions');
    notice('recordings-message', '');
    // Drop preferences for recordings that no longer exist.
    for (const id of [...unticked]) if (!recordings.some(r => r.id === id)) unticked.delete(id);
    store.set('untickedRecordings', [...unticked]);
    renderRecordings();
  } catch (error) { notice('recordings-message', `Couldn't load recordings: ${error.message}`, 'bad'); }
}
window.refreshRecordings = refreshRecordings;

function updateTrainButton() {
  const list = tickedRecordings();
  const missing = missingPoses(list);
  const recording = !!currentCapture?.active;
  byId('start-training').disabled = trainingBusy || recording || !list.length || missing.length > 0;
  if (trainingBusy) notice('train-summary', '');
  else if (recording) notice('train-summary', 'Finish recording first. The new recording will appear here.');
  else if (!recordings.some(r => !r.error)) notice('train-summary', 'Make a recording in step 1 first.');
  else if (!list.length) notice('train-summary', 'Tick at least one recording to train on.', 'warn');
  else if (missing.length) notice('train-summary', `The ticked recordings don't have enough ${listed(missing)}. Record a full set of basic poses, or tick a recording that has them.`, 'warn');
  else {
    const frames = list.reduce((sum, r) => sum + r.frames, 0);
    notice('train-summary', `Ready to train on ${plural(list.length, 'recording')} (${plural(frames, 'frame')}).`, 'ok');
  }
}

function defaultName() {
  return `Trained ${new Date().toLocaleString([], {day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit'})}`;
}
byId('start-training').onclick = async () => {
  const request = {
    name: byId('training-name').value.trim() || defaultName(),
    device: byId('training-device').value,
    epochs: Math.max(1, Math.min(60, Math.round(Number(byId('training-epochs').value) || 12))),
    recordings: tickedRecordings().map(r => r.id)
  };
  byId('start-training').disabled = true;
  try {
    await post('/ext/quest-pro/training/start', request);
    trainingBusy = true;
    byId('training-result').hidden = true;
    showTrainingProgress({message: 'Starting…', fraction: 0});
    showTrainingControls(true);
  } catch (error) {
    notice('training-result', `Couldn't start training: ${error.message}`, 'bad');
    updateTrainButton();
  }
};
byId('cancel-training').onclick = async () => {
  if (!confirm('Cancel training? The model in use stays the same.')) return;
  try { await post('/ext/quest-pro/training/cancel', {}); } catch (error) { notice('training-result', error.message, 'bad'); }
};

function showTrainingControls(busy) {
  byId('training-live').hidden = !busy;
  byId('cancel-training').hidden = !busy;
  byId('start-training').hidden = busy;
}
function showTrainingProgress(progress) {
  const fraction = Math.max(0, Math.min(1, Number.isFinite(progress.fraction) ? progress.fraction : 0));
  byId('training-progress').value = fraction;
  byId('training-percent').textContent = `${Math.round(fraction * 100)}%`;
  const left = Number.isFinite(progress.eta_seconds) ? ` · about ${duration(progress.eta_seconds)} left` : '';
  byId('training-message').textContent = `${progress.message}${left}`;
}
function showFinished(report, switchedOn) {
  const box = byId('training-result');
  box.replaceChildren();
  box.className = 'notice ok'; box.hidden = false;
  box.append(element('strong', switchedOn ? 'Done. Your new model is switched on.' :
    modelOverride ? 'Done. VRFT_TONGUE_MODEL_DIR is set, so the new model was saved but not switched on.' : 'Done. Your new model is saved.'));
  if (report) {
    box.append(element('p', `Trained on ${plural(report.recordings.length, 'recording')} in ${duration(report.seconds)}. ` +
      'Stick your tongue out and move it around: the dot under the camera view should follow.'));
  }
  box.append(element('p', 'Not tracking well? Refit the headset, record another set of basic poses and train again. You can switch back to an earlier model, or the built-in one, under Model in use.'));
}

// Offers the built-in model download until it is installed.
function showBuiltin(builtin) {
  byId('builtin').hidden = !builtin || builtin.installed;
  if (!builtin || builtin.installed) return;
  const button = byId('install-builtin');
  button.disabled = builtin.installing;
  button.textContent = builtin.error ? 'Try again' : 'Download built-in model';
  if (builtin.installing) {
    notice('builtin-message', `Downloading the built-in model… ${Math.round((builtin.fraction || 0) * 100)}%`);
  } else if (builtin.error) {
    notice('builtin-message', `The download failed: ${builtin.error}`, 'bad');
  } else {
    notice('builtin-message', "The built-in model isn't installed yet. VRFaceTracking downloads it once, about 140 MB, from the Qpro-Enhanced-FT release, and checks it before use.");
  }
}
byId('install-builtin').onclick = async () => {
  byId('install-builtin').disabled = true;
  try { showBuiltin(await post('/ext/quest-pro/training/builtin')); }
  catch (error) { notice('builtin-message', `Couldn't start the download: ${error.message}`, 'bad'); }
};

async function refreshTraining() {
  try {
    const state = await getJson('/ext/quest-pro/training/status');
    trainingBusy = state.busy;
    activeModelId = state.active_id;
    modelOverride = state.model_override;
    showTrainingControls(state.busy);
    showBuiltin(state.builtin);
    if (state.busy) showTrainingProgress(state.progress || {message: 'Starting…', fraction: 0});
    else if (state.id && state.progress && shownJob !== state.id) {
      shownJob = state.id;
      const {stage, message} = state.progress;
      if (stage === 'complete') {
        await refreshModels();
        showFinished(state.progress.report, state.active_id === state.id);
        speak('Training complete');
      } else if (stage === 'failed') {
        notice('training-result', `Training failed: ${message}`, 'bad');
      } else if (stage === 'cancelled') {
        notice('training-result', 'Training cancelled. The model in use has not changed.');
      }
    }
    renderModelSelect();
    updateTrainButton();
  } catch (error) {
    byId('training-message').textContent = `Couldn't get the training status: ${error.message}`;
  }
  setTimeout(refreshTraining, 1000);
}

async function refreshModels() {
  const state = await getJson('/ext/quest-pro/training/models');
  savedModels = state.models;
  activeModelId = state.active_id;
  renderModelSelect(true);
}
window.activeModelName = () => {
  const model = savedModels.find(m => m.id === activeModelId);
  return `Tongue model: ${activeModelId === 'demo' ? 'built-in' : model ? model.name : 'trained'}`;
};
function renderModelSelect(force) {
  const select = byId('saved-model');
  select.disabled = trainingBusy || !!currentCapture?.active || modelOverride;
  if (document.activeElement === select) return;
  if (force || select.value !== activeModelId) {
    const personal = savedModels.filter(m => m.id !== 'demo').sort((a, b) => createdAt(b.id) - createdAt(a.id));
    select.replaceChildren(...[{id: 'demo', name: 'Built-in model (not trained on you)'}, ...personal].map(model => {
      const option = element('option', model.name); option.value = model.id; return option;
    }));
    select.value = activeModelId;
  }
  byId('model-message').textContent = modelOverride ? 'Set by VRFT_TONGUE_MODEL_DIR.' : '';
}
byId('saved-model').onchange = async event => {
  const id = event.target.value;
  try {
    await post('/ext/quest-pro/training/activate', {id});
    activeModelId = id;
    byId('model-message').textContent = 'Switched. Tracking starts using it within a second.';
  } catch (error) {
    byId('model-message').textContent = error.message;
    event.target.value = activeModelId;
  }
};

refreshRecordings();
refreshModels().catch(error => { byId('model-message').textContent = error.message; });
refreshTraining();
