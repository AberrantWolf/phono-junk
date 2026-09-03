# TODO

This is the active queue. Historical sprint notes are archived in
[docs/archive/development-history.md](docs/archive/development-history.md).

## Now

- [ ] Establish schema v7 as the first forward-migration baseline.
- [ ] Persist identification and verification evidence independently of the
  resolved catalog projection.
- [ ] Correct dBAR matching and finish offset-aware AccurateRip verification.
- [ ] Introduce `LibrarySession` and an owned, cancellable `JobSupervisor`.
- [ ] Replace flat provider fan-out with staged discovery and candidate scoring.
- [ ] Route CLI and GUI catalog use cases through `LibrarySession`.

## Next

- [ ] Add aggregate list/detail queries and use the same aggregate for export.
- [ ] Finish provider-response reuse for artwork and durable raw-response audit.
- [ ] Add the real redumper/AccurateRip fixture to the opt-in acceptance suite.
- [ ] Exercise a sample v7-to-v8 migration in the migration harness.

## Later

- [ ] Add another Japanese-focused provider after Tower MDB, based on documented
  coverage and responsible-access constraints.
- [ ] Add CTDB as an independent verification source.
- [ ] Extract playback from the GUI if a second consumer appears.
- [ ] Add player mock-backend tests, persistent now-playing chrome, shortcuts,
  and gapless queueing.
