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

## Always

- English for code, comments and commits.
- A test comes with the code it guards, and fails without it.
- Every system call has a bounded duration; no kernel heap; no blocking in the
  kernel.

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
