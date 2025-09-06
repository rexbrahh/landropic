{
  description = "Landropic - Encrypted P2P file sync with full Nix integration";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, crane, ... }:
    let
      # Support systems
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      
      # Version from Cargo.toml
      version = "0.0.1-alpha";
      
      # Shared configuration
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      # Development shells for each system
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          
          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          };

          # Common build inputs
          buildInputs = with pkgs; [
            openssl
            pkg-config
            protobuf
            sqlite
            zlib
          ] ++ lib.optionals stdenv.isDarwin [
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.SystemConfiguration
            darwin.apple_sdk.frameworks.CoreServices
            libiconv
          ];

        in {
          # Default development shell
          default = pkgs.mkShell {
            buildInputs = buildInputs ++ (with pkgs; [
              rustToolchain
              
              # Rust tools
              cargo-watch
              cargo-edit
              cargo-audit
              cargo-outdated
              cargo-nextest
              bacon
              
              # Development tools
              just
              watchexec
              git
              gh
              tokei
              hyperfine
              
              # Nix tools
              nil
              nixpkgs-fmt
              statix
              nix-tree
            ]);
            
            shellHook = ''
              echo "🚀 Landropic Development Environment (Nix)"
              echo "📦 Rust: $(rustc --version)"
              echo "🔧 Cargo: $(cargo --version)"
              echo ""
              echo "Available commands:"
              echo "  just          - Show all tasks"
              echo "  just build    - Build all packages"
              echo "  just test     - Run all tests"
              echo "  just release  - Create release build"
              echo "  bacon         - Watch mode with cargo-bacon"
              echo ""
              export RUST_BACKTRACE=1
              export RUST_LOG=landropic=debug
              export CARGO_BUILD_JOBS=4
              export RUSTC_WRAPPER=""
            '';
          };
          
          # Minimal shell for CI
          ci = pkgs.mkShell {
            buildInputs = [ rustToolchain ] ++ buildInputs;
          };
        });

      # Packages for each system
      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          
          craneLib = crane.lib.${system};
          
          # Common arguments for crane
          commonArgs = {
            src = craneLib.cleanCargoSource ./.;
            buildInputs = with pkgs; [
              openssl
              sqlite
            ] ++ lib.optionals stdenv.isDarwin [
              darwin.apple_sdk.frameworks.Security
              darwin.apple_sdk.frameworks.SystemConfiguration
            ];
            nativeBuildInputs = with pkgs; [
              pkg-config
              protobuf
            ];
          };
          
          # Build dependencies separately for caching
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          
        in rec {
          # Main package - all binaries
          default = landropic;
          
          landropic = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "landropic";
            inherit version;
            
            cargoExtraArgs = "--bins";
          });
          
          # Individual components
          landro-daemon = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "landro-daemon";
            cargoExtraArgs = "--bin landro-daemon";
          });
          
          landro-cli = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            pname = "landro-cli";
            cargoExtraArgs = "--bin landro-cli";
          });
          
          # Docker/OCI image built with Nix
          docker = pkgs.dockerTools.buildLayeredImage {
            name = "landropic";
            tag = version;
            
            contents = with pkgs; [ 
              landropic
              bashInteractive
              coreutils
              cacert
            ];
            
            config = {
              Cmd = [ "/bin/landro-daemon" ];
              WorkingDir = "/data";
              Volumes = {
                "/data" = {};
                "/sync" = {};
              };
              ExposedPorts = {
                "9990/tcp" = {};
                "9990/udp" = {};
              };
              Env = [
                "RUST_LOG=info"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
            };
          };
        });

      # NixOS module for system deployment
      nixosModules.landropic = { config, lib, pkgs, ... }:
        with lib;
        let
          cfg = config.services.landropic;
        in {
          options.services.landropic = {
            enable = mkEnableOption "Landropic sync daemon";
            
            package = mkOption {
              type = types.package;
              default = self.packages.${pkgs.system}.landropic;
              description = "Landropic package to use";
            };
            
            configFile = mkOption {
              type = types.nullOr types.path;
              default = null;
              description = "Path to configuration file";
            };
            
            dataDir = mkOption {
              type = types.path;
              default = "/var/lib/landropic";
              description = "Data directory";
            };
            
            syncDirs = mkOption {
              type = types.listOf types.path;
              default = [];
              description = "Directories to sync";
            };
            
            openFirewall = mkOption {
              type = types.bool;
              default = true;
              description = "Open firewall for QUIC port";
            };
          };
          
          config = mkIf cfg.enable {
            systemd.services.landropic = {
              description = "Landropic Sync Daemon";
              after = [ "network.target" ];
              wantedBy = [ "multi-user.target" ];
              
              serviceConfig = {
                Type = "simple";
                ExecStart = "${cfg.package}/bin/landro-daemon ${optionalString (cfg.configFile != null) "--config ${cfg.configFile}"}";
                Restart = "always";
                RestartSec = 10;
                StateDirectory = "landropic";
                DynamicUser = true;
                
                # Security hardening
                PrivateTmp = true;
                ProtectSystem = "strict";
                ProtectHome = true;
                NoNewPrivileges = true;
                ReadWritePaths = [ cfg.dataDir ] ++ cfg.syncDirs;
              };
              
              environment = {
                RUST_LOG = "info";
                LANDROPIC_DATA_DIR = cfg.dataDir;
              };
            };
            
            networking.firewall = mkIf cfg.openFirewall {
              allowedTCPPorts = [ 9990 ];
              allowedUDPPorts = [ 9990 5353 ];
            };
          };
        };

      # Home Manager module
      homeManagerModules.landropic = { config, lib, pkgs, ... }:
        with lib;
        let
          cfg = config.services.landropic;
        in {
          options.services.landropic = {
            enable = mkEnableOption "Landropic sync for user";
            
            package = mkOption {
              type = types.package;
              default = self.packages.${pkgs.system}.landropic;
            };
            
            syncDirs = mkOption {
              type = types.listOf types.str;
              default = [ "Documents" "Pictures" ];
              description = "Directories to sync (relative to home)";
            };
          };
          
          config = mkIf cfg.enable {
            systemd.user.services.landropic = {
              Unit = {
                Description = "Landropic Sync";
                After = [ "graphical-session-pre.target" ];
              };
              
              Service = {
                Type = "simple";
                ExecStart = "${cfg.package}/bin/landro-daemon --user";
                Restart = "always";
              };
              
              Install = {
                WantedBy = [ "default.target" ];
              };
            };
            
            home.packages = [ cfg.package ];
          };
        };

      # CI/CD checks
      checks = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          # Format checking
          format = pkgs.runCommand "check-format" {} ''
            echo "Format check placeholder"
            touch $out
          '';
          
          # Tests
          test = pkgs.runCommand "run-tests" {} ''
            echo "Test placeholder"
            touch $out
          '';
        });

      # Apps for nix run
      apps = forAllSystems (system: 
        let
          pkgs = nixpkgs.legacyPackages.${system};
          package = self.packages.${system}.landropic;
        in {
          default = self.apps.${system}.daemon;
          
          daemon = {
            type = "app";
            program = "${package}/bin/landro-daemon";
          };
          
          cli = {
            type = "app";
            program = "${package}/bin/landro-cli";
          };
          
          # Development helpers
          dev-setup = {
            type = "app";
            program = toString (pkgs.writeShellScript "dev-setup" ''
              echo "Setting up Landropic development environment..."
              
              # Install pre-commit hooks
              cat > .git/hooks/pre-commit <<'EOF'
              #!/bin/sh
              cargo fmt --check
              cargo clippy -- -D warnings
              EOF
              chmod +x .git/hooks/pre-commit
              
              # Setup direnv
              echo "use flake" > .envrc
              direnv allow
              
              echo "✅ Development environment ready!"
            '');
          };
        });
    };
}