# DodOS kernel

Verification, before every commit and identical in CI:

    nix develop -c ./verify

This repository holds the kernel, its executable model, the oracle harness and
the kernel interface in code (`abi/`). Nothing here knows Wasm or WIT.

The contract is `architecture/kernel-interface.md` in the documentation
repository. The order is never reversed: specification, model, oracle,
implementation.

Skills are in `.agents/skills/`. `zig-0-17` lists the breaking changes of
Zig 0.17 (for example `@cImport` no longer exists); it is copied from
github.com/zigcc/skills at commit 2f9bf9d (MIT). Read it before writing Zig.
`zig-tiger-style` (same source) is the Zig style of the kernel: assertions,
bounded loops, no recursion, sizes asserted at compile time. One exception:
`usize` is the right type for addresses and sizes of memory.
`rust-1-100` lists what changed in Rust 1.97 to 1.100. `dodos-rust` holds
the Rust rules of the project and is the same file in the system repository.

## Always

- English for code, comments and commits.
- A test comes with the code it guards, and fails without it.
- Update `plan/progress.md` in the documentation repository in the same
  change as the work: the colour of the box, the "Now" table, the date. Moving
  a box is part of "done".
- Every system call has a bounded duration; no kernel heap; no blocking in the
  kernel.

## Rust

The rules are in the skill `dodos-rust` (style, tests, lints, what CI runs);
read it before writing Rust. Two more hold in this repository:

- Each branch that applies a rule of the contract names the section of
  `kernel-interface.md` it applies.
- A pure function of `abi/` has a Kani proof (`#[cfg(kani)]`).

## Ask first

- Any change to `abi/`, to the model or to a test: these are the contract and
  are re-read by the two humans.
- A new dependency.
- Any `unsafe` outside the architecture layer.

## Never

- Delete or weaken a test to make `verify` pass.
- Translate or copy code from another kernel. Reading is allowed; cite the
  origin of an idea that is taken.
- Edit the model while implementing against it.
