{
  description = "DodOS kernel: pinned development environment";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  # Rust comes from rust-overlay (the official binaries): nixpkgs is weeks
  # behind a release and does not ship the bare-metal target.
  inputs.rust-overlay.url = "github:oxalica/rust-overlay";
  inputs.rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
      # The machine-readable specifications of rule G11 (D-125), fetched by
      # hash when the shell is entered, never by the build. None is copied
      # into the repository: Arm's package is 208 MB unpacked; the CSR and
      # instruction files of the unified database are 1,814 files and XED's
      # data files 6.3 MB, too large to review in a change.
      specs = pkgs: {
        # Arm AARCHMRS, A-profile, release 2026-09, BSD-3-Clause (docs/notice.html).
        DODOS_SPEC_AARCH64 = pkgs.fetchzip {
          url = "https://developer.arm.com/-/cdn-downloads/permalink/Exploration-Tools-OS-Machine-Readable-Data/AARCHMRS_BSD/AARCHMRS_OPENSOURCE_A_profile_FAT-2026-09.tar.gz";
          stripRoot = false;
          hash = "sha256-KVAkPZ0ml4VEADzr2pPuuTUQxSrOk401xJS46xXH69o=";
        };
        # RISC-V unified database, 13 November 2026, BSD-3-Clause-Clear.
        DODOS_SPEC_RISCV64 = pkgs.fetchFromGitHub {
          owner = "riscv-software-src";
          repo = "riscv-unified-db";
          rev = "d583fecd3d95841b9d6751f800337d1e2999c3fd";
          hash = "sha256-WHOAI1ISapwMWOp5OX9I2wLi8/+D1xo70LV5CJ8ajqk=";
        };
        # The CSR table of the ratified privileged specification, against
        # which the database is checked (CC-BY-4.0).
        DODOS_SPEC_RISCV64_CSRS = pkgs.fetchurl {
          url = "https://raw.githubusercontent.com/riscv/riscv-isa-manual/5f1104a0613af80219232b1e4bc108f9ec2bd6b2/src/priv/csrs.adoc";
          hash = "sha256-Goe3HxY7ZjtEi6bW8lAnXwJX1JKs31coDzi5iflClRA=";
        };
        # Intel XED, 24 August 2026, Apache-2.0.
        DODOS_SPEC_X86_64 = pkgs.fetchFromGitHub {
          owner = "intelxed";
          repo = "xed";
          rev = "0bcb6237345c5066726dcc08b3d87928df3b5b26";
          hash = "sha256-Lhy7q41PM3TXOHJNco/vyTqDtdmE0S+BQJkd7r/vm3c=";
        };
      };
    in
    {
      devShells = forAll (pkgs: {
        default = pkgs.mkShell ({
          packages = [
            ((rust-overlay.lib.mkRustBin { } pkgs).stable."1.100.0".default.override {
              targets = [ "x86_64-unknown-none" ];
            })
            # The Zig half of the kernel (D-126); here it assembles the
            # scanner's test objects and checks the generated Zig file.
            pkgs.zig
          ];
        } // specs pkgs);
      });
    };
}
