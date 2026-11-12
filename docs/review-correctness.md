# Correctness, resource lifetime and performance review

Reviewed the adapters, index lifecycle, transfer collection/writers and child
processes, native resume/delete paths, terminal input/pagination, installer and
release packaging. The following fixes are included in this revision.

## Findings and fixes

1. **Concurrent initialization and migration.** Two commands could read the old
   schema version before either acquired the write lock, then both attempted the
   same CREATE/ALTER. Identity/version checks now use one SQL statement, and
   initialization rechecks them inside the write transaction. Current-schema
   browsing does not acquire a migration write lock. Regression coverage opens
   a new index and a v1 index concurrently from eight threads.
2. **AGY fallback decoding.** `Option::or` eagerly decoded legacy fields even
   when a valid primary user/reply field was present. Invalid legacy bytes could
   make a valid response opaque. Fallbacks now decode only when needed; a test
   includes valid primary text and invalid UTF-8 in the legacy field. AGY's parser
   revision advances to 3 so existing indexes are repaired on sync. Other agents
   keep revision 2 and are not unnecessarily reparsed.
3. **Excess paste allocations.** Search, rename and deletion input copied and
   sanitized the entire pasted string before truncating it. Appending now stops
   at the remaining character budget, without another full clipboard copy. A
   large Unicode paste test covers sanitization and a full destination buffer.
   Crossterm still owns the incoming paste event; this is not a claim that raw
   clipboard input itself has a hard process memory bound.
4. **Helpers surviving successful native commands.** The process guard previously
   skipped cleanup when the direct command exited successfully. A helper in the
   same process group could keep running and retain log descriptors. Cleanup now
   terminates remaining members of the command's own isolated group on success
   as well as failure. The direct child is waited; Linux regression coverage
   verifies that a background sleep does not survive its successful parent.
   This does not attempt to manage unrelated agents or detached process groups.
5. **Final log size check.** A command could write beyond the log limit between
   the polling loop's size check and a successful exit. Size is checked again
   after exit, before accepting output. Runtime/output limits remain in place;
   the temporary log is removed when its file handle is released.
6. **Rename on the last search page.** Renaming away the last title-only match on
   a data page could leave an empty page while earlier matches still existed.
   The selector now moves back one page, as deletion already did. The PTY test
   searches display aliases, goes to the final one-item page and renames away
   that match, then checks the remaining count and corrected offset.

## Performance and caching

No additional historical-result cache, decoded-message cache or background task
was introduced. SQLite's bounded caches and the per-sync AGY summary snapshot
remain: removing those would increase repeated parsing/I/O. Session text is still
streamed during indexing; transfer intentionally collects active-branch text.

A local Linux release-build sample used 1,001 sessions / 120,000 messages, about
47.9 MiB of source data. This was a single run in a shared environment:

| Operation | Elapsed | Observed RSS peak |
| --- | ---: | ---: |
| First sync | 7.23 s | 9.95 MiB |
| Unchanged sync | 13.48 ms | 5.40 MiB |
| List 50 | 4.79 ms | 5.10 MiB |
| List largest 50 | 2.47 ms | Insufficient sampling |
| Preview 20,000-message transfer | 93.93 ms | 34.02 MiB |

RSS is sampled after exec every 2 ms; these figures are not a cross-platform
performance promise or a controlled before/after comparison. The index remains
about 216 MiB due to raw records, normalized text and FTS. No speculative schema
compression or per-delete VACUUM was added.

## Validation and limits

- 54 Rust tests pass on Linux for both GNU and musl targets, including concurrent
  creation/migration, decoder upgrade isolation and process/input regressions.
- Workspace Clippy with warnings denied and release builds pass.
- Both command names pass release PTY checks, including the renamed last page.
- The real OpenCode 1.19.5 deletion probe still removes session/message/part rows,
  preserves DB integrity and cleans the index; no model is invoked.
- Installer/packaging: 12 tests, ShellCheck, workflow actionlint, SQL and native
  fixture verification pass.

The release matrix remains responsible for macOS and Linux ARM64 validation.
These checks do not prove absence of every possible leak or upstream-format bug.
AGY native deletion and the previously documented agent-version limitations are
unchanged.
