# Console screen study

2026-10-04. The purpose is to learn how an operator finds and changes a value,
not to reproduce a brand's artwork or imply equivalent DSP. Sources are pinned
to the particular manuals/screens below; this is not a current feature or buying
comparison. The design conclusions in the final column are our inferences.

## Visual references and decisions

| Console / tier | Screens actually inspected and source | Observation | SHR Desk decision |
|---|---|---|---|
| Behringer WING / WING Compact, accessible full digital desk | HOME screen in a [close product photograph](https://thumbs.static-thomann.de/thumb/padthumb600x600/pics/bdb/_59/599924/19901092_800.jpg); manufacturer [WING family manual, HOME figures](https://cdn.mediavalet.com/aunsw/musictribe/m6Gtew1v9UCRK22GG4TxhA/3ZFYMKP0sESuAFkHeBx8UA/Original/Manual_WING%2C%20WING%20RACK%2C%20WING%20Compact.pdf) and [manufacturer quick-start](https://cdn.mediavalet.com/aunsw/musictribe/mcpCWE3Fb0eDqElYB76giA/BTR7arbWlkSC12MOl1u7jw/Original/QSG_BE_0603-AFC_WING-BK_WW.pdf) | Selected identity across the top; processing sections on the left; input/filter/EQ/level areas; physical encoders immediately below screen | A selected-channel home and persistent status; source identity distinct from channel; adjacent labelled physical assignments. Processing-order editing waits for engine support |
| DiGiCo Quantum 338, high end | Manufacturer [brochure](https://digico.biz/wp-content/uploads/2020/03/Quantum-338-Brochure.pdf), PDF leaves 3 and 5 (printed pp.4–5 and 8–9): channel strips, processor popups, whole-console overview | Repeated vertical strip grammar preserves location; large overview separates orientation from detailed editing | Stable banks and a selected inspector; later group spill/user banks. Do not copy per-send Nodal processing, theatre features or multi-screen density into a one-screen first version |
| Allen & Heath Avantis, high end | [Firmware reference V1.20](https://www.allen-heath.com/content/uploads/2023/05/Avantis-Firmware-Reference-Guide-V1.20.pdf), p.8 Bank View, p.13 PEQ | A bank overview opens a selected processing block; PEQ graph and band values share one screen; processing tabs remain stable | Mix → Channel with selection preserved; curve and precise values together; bypass and unavailable must differ. Reserve graph overlays for a real calibrated feed |
| Soundcraft Vi1, earlier high end | [Manufacturer user guide, issue 0810](https://www.soundcraft.com/en-US/product_documents/soundcraft_vi1_user_guide-pdf), PDF p.35 / printed 3-1, Figure 3-1 | Channel touch fields, EQ/dynamics/sends areas and Vistonics encoder labels are spatially associated | Footer mirrors the MiniLab's actual two rows of eight. The screen tells the hand what each control does; don't rely on memorized hidden pages |
| Soundcraft Ui24R, compact/lower cost | [Manufacturer user guide V1.0](https://www.soundcraft.com/en-US/product_documents/ui24r_manual_v1-0_web-pdf), PDF p.21 / section 3.1 | Scrollable mixer strips, selected-channel focus retained across views, direct transition to EQ | Selected identity persists; bank navigation and detail access work without touch. Browser portability is useful inspiration, not a reason to replace the native full-HD target |
| Yamaha RIVAGE PM, high end | Manufacturer [screen overview](https://manual.yamaha.com/pa/mixers/RIVAGE_PM_series/en-US/7391328779.html), linked [12-channel overview image](https://manual.yamaha.com/pa/mixers/RIVAGE_PM_series/Images/png/5328481291__Web.png) and [selected-channel image](https://manual.yamaha.com/pa/mixers/RIVAGE_PM_series/Images/png/6111843083__Web.png) | Twelve narrow channel strips for orientation; a selected-channel view groups source, EQ, dynamics and sends; identity/menu remains visible | Twelve channels per octave is a strong fit. Keep fast overview and full detail as separate views; use the inspector for frequent edits. Avoid its much denser send matrix in small strips |

WING technical claims above use manufacturer text. The downloaded product photo
is visual evidence only, hosted by Thomann. The full WING manual CDN refused local
download, so its indexed HOME descriptions supplement that inspected photo; a
complete WING routing/scene walkthrough was not performed. Avantis, Vi1, Ui24R and
DiGiCo PDF pages and the two Yamaha images were rendered/opened locally. The small
DiGiCo brochure screenshots support layout observations, not detailed menu claims.
No reference console was operated physically.

As a secondary Yamaha comparison, the official [DM7 feature page](https://usa.yamaha.com/products/proaudio/mixers/dm7/features.html)
also separates Overview and Selected Channel views. A&H SQ is a useful future
compact workflow comparison, but a complete SQ screen/manual review was not
obtained in this pass; no SQ-specific behavior is a design dependency.

## What transfers across price levels

The repeated useful pattern is **overview, selected detail, explicit destination,
visible assignment**. These are observable layout patterns; their suitability for
our controller and viewing distance is still an ergonomic hypothesis.

- Keep channel identity, selected bus, link freshness and recorder state visible
  while editing. A hidden monitor destination is more dangerous than an extra click.
- Separate focus from application. Selecting a strip or opening a processor should
  not change a setting; only an intentional command does that.
- Put exact values next to the graph. A curve is useful for orientation, but an
  engineer needs frequency, dB, Q, attack and release units to make a repeatable edit.
- Align the controller legend with physical positions. MiniLab lacks motorized
  faders and encoder rings, so screen values, pickup and owner indication matter.
- Preserve a short path home. Complex routing, scenes and setup are necessary,
  but they should not displace the ordinary bank/selected-channel workflow.

The console references have substantial DSP and physical infrastructure we do not
have. Their screens cannot establish our engine capabilities. Detailed routing,
PFL, talkback, scene recall, multitrack status and protection only become usable
when their owning module exposes the required contract.

## What our desk needs beyond those references

Automation ownership should be as visible as a mute: applied value, proposal,
reason, allowed bounds, human hold and release action. A conventional console
channel page does not by itself solve manual/automix arbitration. Our Mix draft
therefore gives the selected inspector to ownership and the proposed change.

Tradeoffs to test: twelve narrow strips versus readable long names; the 12-channel
bank crossing the MiniLab's 8+8 rotary rows; controller-only editing versus rapid
page access; continuous meters versus graph refresh budget on a Pi. Prefer a
bounded hardware usability session after D1/D2, with prepared tasks and recorded
observations, over declaring an aesthetic winner from screenshots.

No manufacturer screenshot is bundled into the repository or product. Retain these
source links and concise observations; generated SHR Desk drafts are original
scene output using the licensed project font.
