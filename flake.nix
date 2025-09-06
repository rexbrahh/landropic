{
  description = "Landropic - Encrypted P2P file sync";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Core dependencies
            rustup
            openssl
            pkg-config
            protobuf
            sqlite
            
            # Dev tools
            git
          ];
          
          shellHook = ''
            echo "🚀 Landropic Development Environment"
            echo "Setting up Rust toolchain..."
            rustup default stable
            rustup component add rustfmt clippy rust-analyzer
            echo "Ready!"
            export RUST_BACKTRACE=1
            export RUST_LOG=landropic=debug
          '';
        };
      });
}