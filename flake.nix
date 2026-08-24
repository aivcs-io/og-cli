{
  description = "og-cli: aivcs.io og-cli";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "tarball+https://registry.aivcs.io/api/v1/crates/nix-flake-crane/80db5bdc391be8a1794f6d8a2d56e3a84ebcede2/download";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        src = craneLib.cleanCargoSource ./.;
        commonArgs = {
          inherit src;
          pname = "og-cli";
          version = "0.1.0";
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.apple-sdk ];
          doCheck = false;
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        ogCliBinary = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
        });

      in
      {
        packages.default = ogCliBinary;
        packages.og-cli = ogCliBinary;

        devShells.default = pkgs.mkShell {
          buildInputs = [ rustToolchain pkgs.pkg-config pkgs.openssl ];
        };
      }
    );
}
