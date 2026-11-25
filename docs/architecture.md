# Slowfield architecture

## Product boundary

Slowfield ships one `.scr` executable. A common Rust host owns Windows
screen-saver modes, the native window, one OpenGL 3.3 compatibility context,
timing, configuration, input exit, preview containment, and teardown. Scenes
never create a second window, context, message loop, settings store, or
executable.

## Scene contract

A scene is selected before rendering and has a bounded lifecycle:

1. the host selects an available scene from configuration;
2. the scene initializes only after the host makes its GL context current;
3. the host supplies resize and monotonic elapsed-time updates;
4. the scene renders under its declared cadence and capability policy; and
5. the host deactivates the scene and releases its resources before context
   teardown or a later scene change.

Initial scenes are Prime Field, Gravity Field, and Flock Field. New fields must
fit the same contract and keep all resource ownership inside the one process.

The former standalone implementations are isolated under `legacy/`. They are
read-only behavior references, never child applications or fallback builds.

## Rendering compatibility

The initial common target is an OpenGL 3.3 **compatibility** profile. It is the
minimum common denominator that supports Gravity Field's GLSL 330 program and
allows Prime Field and Flock Field to migrate without combining that migration
with a VBO/core-profile rewrite. Moving all fields to core OpenGL is a separate
future change.

## Frame policies

The host supplies a high-resolution wake-up signal but does not force every
scene to render identically:

- Prime Field derives reveal and zoom directly from time; it performs no
  backlog catch-up.
- Flock Field advances a bounded fixed 60 Hz simulation accumulator.
- Gravity Field preserves one-frame-in-flight GPU fencing and skips work while
  the GPU is busy.

## Configuration

Slowfield owns a fresh per-user configuration schema. It stores the launch
selection policy plus settings for every installed field. On its first valid
load it may import individually valid values from the former Gravity Field and
Flock Field registry keys. It never modifies or deletes legacy keys.

## Non-goals for the initial consolidation

The first release does not dynamically load plugins, run multiple scenes at
once, rotate scenes during one saver session, download assets, use desktop
capture, or require helper processes.
