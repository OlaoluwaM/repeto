{
  description = "Repeto deterministic study-state engine";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ (import rust-overlay) ];
      };
      rustToolchain = pkgs.rust-bin.stable."1.89.0".default.override {
        extensions = [ "clippy" "rust-src" "rustfmt" ];
      };
      rustPlatform = pkgs.makeRustPlatform {
        cargo = rustToolchain;
        rustc = rustToolchain;
      };
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = [ rustToolchain pkgs.pkg-config ];
      };

      packages.${system}.default = rustPlatform.buildRustPackage {
        pname = "repeto";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        meta = {
          description = "Repeto deterministic study-state engine";
          mainProgram = "repeto";
        };
      };

      apps.${system}.default = {
        type = "app";
        program = "${self.packages.${system}.default}/bin/repeto";
        meta.description = "Repeto deterministic study-state engine";
      };

      checks.${system}.quality = rustPlatform.buildRustPackage {
        pname = "repeto-quality";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        doCheck = true;
        checkPhase = ''
          runHook preCheck
          cargo fmt --all -- --check
          cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
          cargo test --workspace --all-targets --all-features --locked
          runHook postCheck
        '';
      };
    };
}
