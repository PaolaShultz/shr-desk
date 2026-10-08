# Owner EQ v1 coefficient/response corpus

Twelve actual shared-library readbacks cover 8000/48000/192000Hz and bypass,
bell, low shelf and high shelf profiles, GEQ Q4.318, rate-clipped identity bands,
Q extremes and shelf S extremes. Each file records actual current/target settings,
coefficient order, denominator, sample rate, independent graph/EQ generations,
settings fingerprints and target response samples. SHA256SUMS pins fixture bytes.
The producing shared library SHA256 is
`7bd1cfcb950961d8106dcdcaa21c2ed05abc7afe052d628e81f06fa85b087e91`.
Producing source pins are recorded separately after the implementation commit.

The banks come from `shr_pa_eq_v1_readback`, not another EQ implementation.
Python complex frequency evaluation over those exact rows generated response samples;
normal `owner_eq_fixtures` replays settings through the owner and compares independent
real/imaginary transfer calculations. During transitions these are separate static
current/target curves, never a measured compressor/system transfer function.
These software fixtures do not establish physical response or audio deadlines.
