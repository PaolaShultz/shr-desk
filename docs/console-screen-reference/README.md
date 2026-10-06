# Audio console screen references

Retained design-reference library, reviewed **2026-10-06**: **12 images**, each individually
inspected and accompanied by a detailed description. Open [the offline gallery](index.html)
to browse pictures and all observations together, or use the per-screen Markdown links below.
The gallery is static HTML with local images; it needs no server, build, scripts or external fonts.

[Lighting companion library](https://github.com/PaolaShultz/shr-lightdesk/tree/main/docs/console-screen-reference) · [existing console study](../CONSOLE_STUDY.md) ·
[owning screen plan](../SCREENS.md) · [source register](SOURCES.md) · [manifest](manifest.json)

## How to use this library

1. Find a workflow in the comparison table and open the relevant full-resolution figure.
2. Read its visible-layout/control description before drawing conclusions from colour alone.
3. Use the design implications as hypotheses for original SHR Desk layouts, checked against
   the owning screen plan, runtime capabilities and actual controller/display review.
4. Preserve the difference between observed pixels, source-documented context and our proposals.

The relevant planned areas are Mix, Channel, Analysis, monitor sends, routing and future scene/health pages. No product code or screen plan
is changed by this collection. The historical study's statement that no screenshots were retained
describes the October 4 review; this separately requested October 6 folder now retains selected figures.

## Workflow comparison

| Design question | Screen IDs | Pattern to consider |
|---|---|---|
| Bank and selected detail | 01, 02, 03, 09, 10, 12 | Stable channel IDs and bank positions; preserve selected identity when changing pages. |
| EQ and analysis | 04, 08 | Curve plus exact Hz/dB/Q; separate computed response from a labelled real measurement feed. |
| Dynamics | 05 | Configured parameters, processor enable, applied state and actual gain reduction are separate. |
| Physical/logical routing | 06 | Source port and logical channel are different axes; inspect before a reviewed patch action. |
| Faders and audition | 07, 09, 11 | Readable destination, level and mute; distinguish selection, PFL/AFL and audience-path edits. |
| System/scene overview | 10 | Global orientation can be dense; ordinary editing should remain in a readable bank/detail view. |

## Screen catalogue

| ID | Console / screen | Native size | Reference role |
|---|---|---|---|
| 01 | [Yamaha RIVAGE PM — 12-channel Overview](screens/01-yamaha-rivage-overview.md) | 850×475 | Orient across a bank of twelve audio channels without opening twelve separate editors. This is a processing overview rather than a large-fader mixing page. |
| 02 | [Yamaha RIVAGE PM — Selected Channel View](screens/02-yamaha-rivage-selected-channel.md) | 1018×620 | Inspect most of one selected audio channel on a single page, while retaining navigation to system operations and bus sends. |
| 03 | [Allen & Heath Avantis — Input-channel Bank View](screens/03-avantis-bank.md) | 1180×659 | Survey an input bank using miniature processing summaries, with a persistent selected channel and active mix context. |
| 04 | [Allen & Heath Avantis — Four-band parametric EQ](screens/04-avantis-peq.md) | 1016×777 | Edit a four-band parametric EQ while comparing its combined response with a labelled analysis overlay. |
| 05 | [Allen & Heath Avantis — Compressor and sidechain](screens/05-avantis-compressor.md) | 1037×794 | Present a compressor as a complete processing block, including its trigger source, timing, transfer curve and parallel-path controls. |
| 06 | [Allen & Heath Avantis — Local input patch matrix](screens/06-avantis-input-patch.md) | 1037×794 | Show physical input-to-logical-channel assignments in a matrix while separating browsing from an explicit patch operation. |
| 07 | [Soundcraft Ui24R — Tablet Mix page](screens/07-ui24r-mix.md) | 395×237 | Compare a compact browser/tablet mixer with the denser large-console bank views. |
| 08 | [Soundcraft Ui24R — Parametric EQ page](screens/08-ui24r-eq.md) | 493×278 | Show how a smaller mixer keeps the selected-channel fader and master context while a large EQ graph is open. |
| 09 | [DiGiCo Quantum 338 — iPad control Mix view](screens/09-digico-ipad-mix.md) | 908×644 | Compare a remote-control mixer page that devotes most of its area to large, direct fader controls. |
| 10 | [DiGiCo Quantum 338 — System overview and snapshots](screens/10-digico-system-overview.md) | 1016×745 | Survey a large console across multiple channel and bus families while keeping snapshot information in the same workspace. |
| 11 | [DiGiCo Quantum 338 — Solo buses and True Solo source selection](screens/11-digico-solo-routing.md) | 754×604 | Inspect dedicated solo-bus controls and select a processing source without hiding the fact that audition routing has its own context. |
| 12 | [Yamaha DM7 — Overview and Selected Channel comparison](screens/12-yamaha-dm7-overview-and-detail.md) | 770×540 | Compare the overview/detail relationship in a newer Yamaha visual language within one manufacturer promotional composite. |

## Scope and limits

These are complete available source figures, sometimes cropped windows, teaching annotations or
promotional composites. They are not all full-desktop captures. Small original images remain small;
no invented detail or upscaling fills missing text. Versions identify source material, not latest
firmware. No reference console was physically operated. No screenshot proves our own engine,
measurement, device connection, cue timing or output capability. Manufacturer images remain
third-party reference evidence; see [source status](SOURCES.md).

WING and Soundcraft Vi1 were considered from the existing study, but their primary manual
endpoints did not provide downloadable figures in this pass. They are omitted rather than
substituting unverified thumbnails. Retained audio families are RIVAGE PM, DM7, Avantis, Ui24R
and Quantum 338, with the Quantum iPad illustration explicitly identified.
