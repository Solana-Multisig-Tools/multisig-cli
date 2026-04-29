{
  description = "Solana dev environment for the msig CLI";
  inputs = {
    nixpkgs.url =
      "github:NixOS/nixpkgs/6b70ae9e4f9738d69a7f1e5cdf05415ce233e358";
    flake-parts.url =
      "github:hercules-ci/flake-parts/3107b77cd68437b9a76194f0f7f9c55f2329ca5b";
    rust-overlay.url =
      "github:oxalica/rust-overlay/146e7bf7569b8288f24d41d806b9f584f7cfd5b5";
  };
  outputs = inputs@{ self, nixpkgs, flake-parts, rust-overlay }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems =
        [ "aarch64-darwin" "aarch64-linux" "x86_64-darwin" "x86_64-linux" ];
      perSystem = { config, self', inputs', pkgs, system, ... }:
        with import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        let
          sharedBuildInputs = [ libiconv pkg-config gcc openssl ];
          rustStable = rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = makeRustPlatform {
            cargo = rustStable;
            rustc = rustStable;
          };
          msig = rustPlatform.buildRustPackage {
            pname = "msig";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = sharedBuildInputs;
            buildInputs = [ openssl ]
              ++ lib.optionals stdenv.isDarwin [ libiconv ];
          };
        in {
          devShells.default = mkShell {
            packages = [ rustStable ];
            buildInputs = sharedBuildInputs;
          };
          packages.msig = msig;
          packages.default = msig;
        };
    };
}
