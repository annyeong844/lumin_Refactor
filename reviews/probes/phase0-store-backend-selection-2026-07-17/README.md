# Phase 0 Store Backend Selection

Status: **PASS - select exact `redb 4.1.0`; overall Phase 0 remains blocked**

## Decision

Architecture v1 selects exact `redb 4.1.0` as the only production persistence
backend. Bundled SQLite through exact `rusqlite 0.39.0` remains comparison evidence
only. Lumin will not ship dual backends, a runtime selector, or a fallback database.

The decision is bound to architecture commit
`65e60216891bb3d826a4778f84cb8aaa377abe92`, architecture manifest
`66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`, and probe
source manifest
`30be1edac70bb27f78c626bf7099aee1f825f2a0f59797a72b9acdc781e9725e`.

## Correctness Gate

Both candidates passed every frozen oracle before ranking:

- 640 conflicting/disjoint cross-process admission rounds;
- 20 forced-death admission cases;
- 470 backend/fault cases;
- 190 namespace cases, of which 170 were injected and detected after displacement
  and 20 were prevented by Windows before displacement.

The evidence spans Windows x64/NTFS, WSL2 ext4 GNU/musl, and native non-WSL Linux
ext4 GNU/musl. No candidate correctness failure was observed.

## Measured Comparison

| Host/mode | Query p50 redb / SQLite | Durable p50 redb / SQLite | Binary redb / SQLite | RSS redb / SQLite | Store redb / SQLite |
| --- | ---: | ---: | ---: | ---: | ---: |
| Windows MSVC | 12,133 / 13,758 us | 15,236 / 21,770 us | 2,333,696 / 3,247,104 B | 15,257,600 / 13,189,120 B | 8,425,472 / 3,182,592 B |
| WSL2 GNU | 14,117 / 1,017 us | 17,286 / 22,849 us | 2,158,264 / 3,458,008 B | 12,451,840 / 12,144,640 B | 8,425,472 / 3,182,592 B |
| WSL2 musl | 14,360 / 879 us | 16,307 / 21,081 us | 2,143,032 / 3,173,744 B | 22,544,384 / 16,777,216 B | 8,425,472 / 3,182,592 B |
| Native GNU | 473 / 357 us | 557 / 1,394 us | 2,158,424 / 3,458,200 B | 13,004,800 / 12,320,768 B | 8,425,472 / 3,182,592 B |
| Native musl | 481 / 402 us | 577 / 1,387 us | 2,143,256 / 3,173,920 B | 23,576,576 / 16,240,640 B | 8,425,472 / 3,182,592 B |

redb won durable-admission p50 and release-binary size in all five comparisons. It
also used 12-13 fewer transitive packages and bundled no native C source. SQLite won
four of five bounded-query p50 comparisons, all peak-RSS comparisons, and store size.

The decisive product priorities are the durable write-gate mutation path and a small,
self-contained native Rust distribution. Query latency remains bounded under both
candidates and the native-host gap was 79-116 us at p50; SQLite's larger WSL query
advantage is preserved as evidence but does not outweigh the mutation and delivery
surface. Store size and RSS remain inputs to the later numeric-budget approval.

## Evidence Bindings

| Packet | Evidence manifest SHA-256 | Summary SHA-256 |
| --- | --- | --- |
| Windows x64/NTFS | `6d404cfc4b25ed581a9f021fc6248c6e7a94c2fdc38b668872c009c0f747ef2d` | `6623d3425294bcf8d5949348dd660a9fa9762d7808a453cb0f1b60df9ea385b3` |
| WSL2 ext4 GNU/musl | `c65a9224bfd03482ea947661c70edc7168d1662a18eb1a0447604fa58f807b3e` | `f12f91a02c3f15f2525b476c330839351054d23fe52176c91fc9adf28d9ff81f` |
| Native Linux ext4 GNU/musl | `0544d252540e14b8f9392d8a83a37209748af0995d6dd2240397ec4941e75046` | `49c3a4f7bc7191ee85ae2f3ce15bec35f4bf48a17619f7574598794dd951e5a9` |

The machine-readable decision is [selection.json](./selection.json). The native
packet additionally binds GitHub Actions run `29584914108`, runner commit
`0b5988c8176c73e9d6d8936cbcc90eebcac3c2a5`, and artifact SHA-256
`9ffc3fd385c1d6b8af748eda20c26f623f4a18420a3e9a540cb91b6f0f7706e4`.

## Boundary

This closes the Architecture v1 store-backend correctness, measured-comparison, and
single-selection gate. It does not approve Phase 1 budgets or the overall Phase 0
freeze. The architecture amendment that records this selection requires a new exact
Git identity and independent review before freeze.
