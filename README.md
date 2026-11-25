# Slowfield

*A quiet collection of slow generative fields for Windows.*

Slowfield will become one self-contained Windows screen saver, `slowfield.scr`.
It begins with three independent visual systems that are being consolidated into
one Rust application:

- **Prime Field** — an Ulam-prime spiral whose spacing follows the continuous
  natural-number frontier.
- **Gravity Field** — a procedural Schwarzschild-style black-hole scene.
- **Flock Field** — a fixed-step predator/prey flocking simulation.

The working default will choose one available field randomly at launch. The
configuration UI will also permit selecting a single field explicitly. The
first consolidation release does not change fields during an active saver
session.

## Status

The repository is in a controlled migration. The existing Gravity Field C
screensaver remains the buildable baseline while the shared Rust host is
introduced. Historical source from Prime Field and Flock Field lives under
`legacy/` only as a migration reference; it is not built or shipped by
Slowfield.

The detailed migration sequence is tracked in
[issue #1](https://github.com/qink15b73/slowfield-screensaver/issues/1).

## Planned product contract

- one Windows process, one Win32 host, one OpenGL 3.3 compatibility context;
- standard `/s`, `/p <HWND>`, `/c`, `/w`, and `/d` modes;
- one per-user configuration schema with safe import from valid legacy values;
- no runtime shader, texture, model, DLL, network, or helper-executable
  dependency;
- bounded scene workloads and scene-specific frame policies; and
- one release artifact: `slowfield.scr`.

## Current baseline build

Until the Rust workspace takes over the release artifact, the retained Gravity
Field baseline can be built with:

```bat
build.bat
```

It currently produces `blackhole.scr`. This is transitional and will be
replaced only after all scenes run from the shared Rust host.

## Repository layout

```text
legacy/                 Imported source retained for behavior comparison
  boids-screensaver/    Archived Flock Field source
  ulam-screensaver/     Prime Field source without prior Git history
docs/                   Architecture, provenance, and retained documentation
```

## License and notices

Slowfield is MIT-licensed. Existing attribution for the initial Gravity Field
shader and the Flock Field reference is retained; see `LICENSE`, `NOTICE.md`,
and `docs/provenance.md`.
