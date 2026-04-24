{
  description = "pds — persistent data structures";

  inputs = {
    nixpkgs.url     = "github:nixos/nixpkgs/nixpkgs-unstable";
    rust-overlay    = {
      url            = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays      = [ (import rust-overlay) ];
        pkgs          = import nixpkgs { inherit system overlays; };

        # Stable toolchain for normal development.
        stableToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rustfmt" "rust-src" "rust-analyzer" "clippy" "llvm-tools-preview" ];
        };

        # Nightly toolchain for miri and cargo-fuzz.
        nightlyToolchain = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [ "miri" "rust-src" ];
        };
      in {
        devShells = {
          # Default shell — stable toolchain for everyday development.
          default = pkgs.mkShell {
            packages = [
              stableToolchain
              pkgs.sccache
              pkgs.cargo-audit
              pkgs.cargo-llvm-cov
              pkgs.samply           # CPU profiling — `samply record cargo bench ...`
              pkgs.cargo-flamegraph # Flamegraph generation from perf/dtrace data
            ];
            # Route compilations through the shared sccache instance.
            RUSTC_WRAPPER = "${pkgs.sccache}/bin/sccache";
            shellHook = ''
              export RUST_BACKTRACE="''${RUST_BACKTRACE:-1}"
              export RUST_LOG="''${RUST_LOG:-warn}"
              # Override global ~/.cargo/config.toml rustflags which includes
              # -Z threads=14 (nightly-only). RUSTFLAGS env var takes precedence
              # over config-file build.rustflags. Keep lld for fast linking.
              export RUSTFLAGS="-C link-arg=-fuse-ld=lld"
            '';
          };

          # Nightly shell for miri and fuzzing (entered via `nix develop .#nightly`).
          nightly = pkgs.mkShell {
            packages = [ nightlyToolchain pkgs.cargo-fuzz ];
            shellHook = ''
              export RUST_BACKTRACE="''${RUST_BACKTRACE:-1}"
            '';
          };
        };
      });
}
