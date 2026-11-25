# Branch integration and plan v12

The implementation notes below identify documented evidence, not proof of
device compatibility or of the source history of generated code. External
sources are used for wire facts and ideas; source code must not be copied (v12
§0.5). All hardware checks listed below remain open.

| Area | Plan / reference | What is supported by the branch evidence | Open work |
| --- | --- | --- | --- |
| AP1 sender | v12 §1.1, §2 phase 3: OwnTone, D19/D33/D47/D48 | Local fake receiver and escape ALAC tests. OwnTone `src/outputs/raop.c` and `src/outputs/rtp_common.c`, commit `612d6b3cf34ae38bb71b6edfcd69fd7afe6ab25c`, were checked for transport, sync and key-wrapping wire facts. RSA-OAEP-SHA1 wraps the AES key using the public RAOP modulus; audio sync uses a caller-supplied or current local timestamp. | No independent ALAC vectors, Apple-Response verification, independent RSA wire fixture, fan-out validation or hardware test. Timing depends on correct caller pacing and the same clock for supplied timestamps. |
| AP1 receiver | v12 §1.1 and phase 2: shairport-sync, D23/D24/D25/D43/D44/D45/D48 | `RAOP-RX-20261125.md` records synthetic SDP, RTP, PCM and timing tests. | Only unencrypted input; RSA/FairPlay and resend requests are not implemented. The branch does not identify a shairport-sync revision or recorded wire comparison; iPhone, iTunes and Mac are untested. |
| Mirroring TX | v12 phase 5 D10/D18/D20/D22/D43/D51/D70–D73 | `MIRROR-TX-20261125.md` identifies doubletake commit `5e2f1cbe…`; tvpipe is derived, not independent. Loopback verifies the local protocol. | No Apple TV validation; geometry, FairPlay requirements, audio retransmission and PTP remain open. RX and TX acceptance are separate. |
| MRP card | v12 phase 6 D12/D13/D28/D40/D41/D55 | Local reconnect, metadata and artwork tests. | The branch does not record a D55 golden-vector comparison or an iPhone check; do not mark phase 6 complete. |
| Photo | v12 phase 8 D75/D88/D90/D91 | `PHOTO-PROFILE-PHASE8-20261125.md` attributes protocol observations to openairplay; profile is opt-in. | Independent wire vectors, iPhone validation, slideshow support and full phase acceptance remain open. |
| Discovery / multi-profile / group / RTP / latency / metadata / reconnect / input robustness | v12 phases 0, 3–6 and S1–S8 as applicable | Synthetic tests and limitations are documented in the individual branch notes. | The notes do not establish which v12 references or captured vectors were used for every wire claim. Hardware, acoustic sync and independent protocol checks remain open. |
| Extended display | v12 phase 5 | `EXTENDED-DISPLAY-20261125.md` cites UxPlay and Wormhole observations. | The prior default `displays[].features=2` is retained; `0xE` is opt-in. Extended desktop and iOS regression need physical testing. |

The integration does not claim that the original prompts named all required
v12 references. Missing provenance or cross-checks are follow-up work, not a
reason to present an experimental profile as complete.
