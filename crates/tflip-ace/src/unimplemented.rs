//! The functions of `ace.h` that tflip does not implement and that have no
//! module of their own, so that a client resolving every function of the
//! interface finds them all. Each takes the parameters `ace.h` declares (a
//! 32-bit `stdcall` function pops its own arguments, so they must match),
//! logs on the `fixme` channel the first time it is called, and fails with
//! `AE_NOT_IMPLEMENTED`. What tflip lacks is listed in
//! `docs/KNOWN_ISSUES.md`.
//!
//! No function is left to list here.
