# Training tongue models on a rented GPU

For experiments that need many training runs, such as comparing synthetic
sets: each run gets its own GPU on a rented Linux server, so four runs take
about as long as one.

## The server

Any Linux machine with NVIDIA GPUs, SSH access and CUDA 12.8 or later works
(for example a RunPod pod from a CUDA 12.8 "devel" template, which has the
NVRTC library the GPU kernels are compiled with). Training uses Burn's CUDA
backend there (the `cuda` feature of `vrft-tongue`) rather than wgpu, since
servers rarely have a Vulkan driver.

## Use

From the repo root in Git Bash, with the server's SSH details in the
environment (never in the repo):

```bash
export VRFT_REMOTE=root@<host> VRFT_REMOTE_PORT=<port>
tools/tongue-remote/remote.sh setup                       # copy the source, build the tools
tools/tongue-remote/remote.sh data models models/quest-pro
tools/tongue-remote/remote.sh data real .local/tongue-captures/<recording>
tools/tongue-remote/remote.sh data synth .local/tongue-synth-runs/<set> ...
tools/tongue-remote/remote.sh run jobs.txt                # train one job per GPU, then score
tools/tongue-remote/remote.sh fetch .local/tongue-remote  # reports, scores and model pairs
```

A jobs file names the recording every job is scored on, the model pair
every job starts from, then one job per line: name, passes, learning rate,
layers (`all`, `head` or `output`) and comma-separated recordings, all
relative to the uploaded `data` folder:

```
test real/<recording>
base models/quest-pro
gentle 12 1e-5 all synth/<set>
default 12 1e-4 all synth/<set>,synth/<another set>
```

Jobs that finished before are scored again but not retrained, so a run
can be repeated after adding jobs.
