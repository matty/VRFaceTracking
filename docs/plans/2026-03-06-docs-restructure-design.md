# Docs Restructure Design

**Date:** 2026-03-06
**Status:** Approved

## Problem

The current flat `docs/` structure mixes content aimed at three distinct audiences — end users, avatar creators, and developers — with no clear separation. Key issues:

- No dedicated end-user getting-started guide (users currently directed at a configuration reference)
- Avatar creator parameter reference doesn't exist; they're left to infer from developer docs
- `resonite.md` reads as a generic OSC tutorial rather than project-specific integration notes
- Stale content: `vrc_parameter_pipeline.md` references removed function `create_unified_expression_params()`
- Calibration described as "min/max scaling" in multiple docs when the algorithm uses mean + std_dev
- Glossary missing EParam, SRanipal, OSCmooth entries

## Approved Structure

```
README.md                          # Redesigned landing page with audience paths
docs/
  guide/                           # End users (download from GitHub Releases)
    getting-started.md             # NEW: download → install → configure → run
    configuration.md               # MOVED from docs/configuration.md
  avatars/                         # Avatar creators
    v2-parameters.md               # NEW: unified expression parameter reference
  internals/                       # Developers
    architecture.md                # MOVED + fix calibration description
    creating-a-module.md           # MOVED
    mutation-pipeline.md           # MOVED + fix calibration description
    vrc-parameter-pipeline.md      # MOVED + remove stale reference
    resonite.md                    # MOVED + rewritten as WIP dev notes
    eye-tracking-analysis.md       # MOVED
  glossary.md                      # STAYS (cross-cutting)
  plans/                           # Design docs (this file)
```

## README Changes

- One-paragraph description of what vrft_d is and does
- Hardware support: Virtual Desktop (native), VRCFaceTracking modules (.NET runtime)
- Platform: Windows only
- Three explicit audience sections linking into the right docs subfolder
- Minimal inline quick-start; full detail in `docs/guide/getting-started.md`

## New Files

### `docs/guide/getting-started.md`
- Download from GitHub Releases
- Place tracking module in correct plugins folder
- Minimal `config.json` setup
- Run `vrft_d.exe`
- What to expect (logging, OSC output)

### `docs/avatars/v2-parameters.md`
- What v2 parameters are and how vrft_d sends them
- Full unified expression list with OSC address, type, and range
- Eye, brow, mouth, tongue categories
- Note on legacy/SRanipal parameters (internal only, not for new avatars)

## Moved + Modified Files

### `docs/guide/configuration.md` (from `docs/configuration.md`)
- No structural changes; keep config reference table and debug API

### `docs/internals/architecture.md` (from `docs/architecture.md`)
- Fix calibration description: "mean + std_dev based normalization with confidence blend" not "min/max scaling"

### `docs/internals/mutation-pipeline.md` (from `docs/mutation_pipeline.md`)
- Fix CalibrationMutation description: same as above

### `docs/internals/vrc-parameter-pipeline.md` (from `docs/vrc_parameter_pipeline.md`)
- Remove reference to `create_unified_expression_params()` (function no longer exists)
- Update to reflect current registry structure

### `docs/internals/resonite.md` (from `docs/resonite.md`)
- Strip generic OSC tutorial content (socket setup, encoding, C# pseudocode)
- Replace with: WIP status note, what the Resonite output strategy does, known gaps

### `docs/internals/creating-a-module.md`, `eye-tracking-analysis.md`
- Move only, no content changes needed

## Glossary Additions

- **EParam**: Container parameter that emits a bool, float, and optional binary sub-param for a single expression
- **SRanipal**: HTC's legacy lip/eye tracking SDK; vrft_d translates unified expressions to SRanipal-compatible shapes for backwards compatibility
- **OSCmooth**: VRChat community tool for smoothing OSC parameters; vrft_d's binary parameter prefix matching supports OSCmooth's address format
- **V1 / Legacy Parameters**: SRanipal-era parameter names, supported for avatar backwards compatibility only
- **V2 Parameters**: The unified expression parameter set (`v2/` prefix); recommended for all new avatars
