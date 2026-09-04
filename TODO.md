# TODO

This is the active queue. Historical sprint notes are archived in
[docs/archive/development-history.md](docs/archive/development-history.md).

## Now

- [ ] Run the opt-in 18-track Redumper/AccurateRip acceptance test when the
  external package is mounted, and record the resulting inferred shift.
- [ ] Confirm the new macOS and Ubuntu CI jobs on the first pushed branch.

## Next

- [ ] Add a GUI evidence inspector for provider observations, candidate scores,
  ambiguity records, and per-track verification results.
- [ ] Add explicit per-job cancellation controls on top of session-wide joined
  shutdown if the UI grows concurrent read-only jobs.
- [ ] Make the first post-v7 schema change only through the exercised forward
  migration harness.

## Later

- [ ] Add another Japanese-focused provider after Tower MDB, based on documented
  coverage and responsible-access constraints.
- [ ] Add CTDB as an independent verification source.
- [ ] Remove the provider-result compatibility adapter once consensus consumes
  `ReleaseCandidate` directly.
- [ ] Extract playback from the GUI if a second consumer appears.
- [ ] Add player mock-backend tests, persistent now-playing chrome, shortcuts,
  and gapless queueing.
