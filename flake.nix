{
  description = "Dev environment for the msig CLI";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay.url = "github:oxalica/rust-overlay";
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
