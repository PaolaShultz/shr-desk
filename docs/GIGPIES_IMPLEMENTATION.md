# SHR Desk: plan and implementation

SHR Desk owns audio actions, controller translation and native presentation.
GigPies owns audio processing, authority and device/transport integration.

## Delivered contract and evidence

Provider-backed native controls, four-band channel processing, dynamic banking,
reviewed PA/output edits and Brain duplex/PFL/AFL/talkback controls have recorded
software acceptance. [Native contract](NATIVE_FRONTEND.md),
[dated evidence](STATUS.md), and GigPies modular/Brain acceptance define the scope.
Physical display/controller/LED operation is unqualified. Real level meters,
writable FX and scene workflows must not be inferred from simulator pictures.

## Next work

- Measured audio presentation is a contribution to
  [GP-METER](https://github.com/PaolaShultz/gigpies/blob/main/docs/MODULE_IMPLEMENTATION_PLAN.md#gp-meter--measured-audio-on-the-real-desk).
  Keep its end-to-end plan and progress there; no separate DS-METER tracker.
- FX, recording and scene UI contributions follow the corresponding shared cards.
- Physical console work follows GP-H2. Module-only learned-profile or renderer
  work gets one card here only when selected, with actual provider/device bounds.

[Screen design](SCREENS.md) specifies intended workflows; it is not a task queue.

## Tracking and verification

This is the owning plan for module-only GigPies work. Keep each new task's plan,
implementation state, checklist, evidence and next action together here. Shared
integration tasks live only in the [GigPies integration plan](https://github.com/PaolaShultz/gigpies/blob/main/docs/MODULE_IMPLEMENTATION_PLAN.md);
link to their cards instead of copying status. Follow its task lifecycle and this
repository's AGENTS.md. STATUS/acceptance files hold dated evidence, not another queue.
Preserve closed milestone details in the linked archive; do not reopen old launch cards.
A documentation reconciliation does not rerun tests or qualify hardware.

[Historical plan and milestone evidence](archive/tracking-before-2026-10-09/GIGPIES_IMPLEMENTATION.md). Its launch instructions are retired.
