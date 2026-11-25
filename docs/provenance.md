# Source provenance

Slowfield consolidates independently developed Windows screen savers. The
imported directories are reference material during a behavior-preserving Rust
migration; they are not nested repositories and are not part of the release
build.

## Gravity Field

- **Origin:** `qink15b73/blackhole-screensaver`
- **Preservation:** this repository was renamed to
  `qink15b73/slowfield-screensaver`, retaining its Git history.
- **Baseline commit:** `c25c432` — `fix: update scene catalogue to 864 scenes
  and adjust inclination count to 4`.
- **Imported reference:** `legacy/gravity-field/`.
- **License and attribution:** MIT. The existing attribution for adapted
  initial shader material from `s0xDk/ghostty-blackhole` remains in `LICENSE`.

## Flock Field

- **Origin:** `qink15b73/boids-screensaver`
- **Baseline commit:** `b193491` — `Hide cursor in full-screen screensaver`.
- **Repository status:** archived after import; its remote history remains
  available for audit.
- **Imported reference:** `legacy/boids-screensaver/`.
- **License and attribution:** MIT. The project notice for the
  `rystrauss/boids` reference is retained at
  `legacy/boids-screensaver/NOTICE.md` and must be carried into Slowfield's
  final notice before the field is shipped.

## Prime Field

- **Origin:** local `D:\Coding\ulam-screensaver` working directory.
- **Repository status:** it did not have a Git repository or remote history.
- **Imported reference:** `legacy/ulam-screensaver/`.
- **License:** MIT, copyright year 2026.
- **Imported snapshot:** 2026-11-25. The source includes Ulam-coordinate,
  prime-sieve, continuous natural-number reveal, and circular inradius rules
  that the Rust migration must preserve.

## Import boundaries

- Build directories, release binaries, and Rust compilation output are retained
  locally for reference but ignored by Slowfield Git.
- The archived Flock Field nested `.git` directory was deliberately removed so
  Slowfield remains the only Git worktree under this root.
- Legacy source is not compiled by Slowfield. The shared Rust workspace ports
  fields incrementally after behavior-specific verification succeeds.
