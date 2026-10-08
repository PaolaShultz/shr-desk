# GP20 measurement producer corpus

`producer.json` is the exact synthetic actual-owner exchange recorded by GigPies
at frozen wire/document commit `832e82673f8bbcf152b9bb64416eb81ba16d56e7`.
SHA-256: `812fe38e80dc6dd6dfa2ada6232b36b1663481bb4978c67879298a2771f72f58`.

It contains correlated snapshot, pending/final capture/proposal/cancel and detailed
owner results across two positions. No microphone or physical output was opened.
Nested owner documents are opaque finite-number JSON strings; the outer envelope
remains integer-only. Desk validates these documents independently of its shared
transport decoder and never computes alignment or generates an owner candidate.

This fixture protects compatibility and strict decoding. The ignored
`measurement_actual_frontend` test separately exercises the real keyboard,
provider connection, PA owner results, whole-configuration review and application.
