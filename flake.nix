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
    in
    {
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = [
            ((rust-overlay.lib.mkRustBin { } pkgs).stable."1.100.0".default.override {
              targets = [ "x86_64-unknown-none" "x86_64-unknown-uefi" ];
            })
          ];
        };
      });
    };
}
