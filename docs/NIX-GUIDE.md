# Landropic Nix Guide

## 🚀 Quick Start

### Installing Nix
```bash
# macOS/Linux
curl -L https://nixos.org/nix/install | sh

# Enable flakes (one-time)
echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

### Development Environment
```bash
# Enter dev shell with all tools
nix develop

# Or use direnv for automatic loading
direnv allow
```

## 📦 What Nix Provides

### Development Shells

1. **Default Shell** (`nix develop`)
   - Full Rust toolchain with all targets
   - Cargo tools (watch, audit, nextest, etc.)
   - Development utilities (just, bacon, gh)
   - Debugging tools (gdb, lldb, valgrind)
   - Network debugging (wireshark, tcpdump)

2. **CI Shell** (`nix develop .#ci`)
   - Minimal environment for CI/CD
   - Just Rust toolchain and dependencies

3. **Benchmarking Shell** (`nix develop .#bench`)
   - Performance profiling tools
   - Flamegraph generation
   - Criterion benchmarking

### Build Targets

```bash
# Native build
nix build                    # Default package
nix build .#landro-daemon   # Just daemon
nix build .#landro-cli      # Just CLI

# Cross-compilation
nix build .#landropic-static         # Static Linux binary
nix build .#landropic-aarch64-linux  # ARM64 Linux
nix build .#landropic-windows        # Windows (experimental)

# Docker
nix build .#docker          # OCI-compliant image
```

## 🎯 Common Workflows

### Daily Development
```bash
# Enter dev environment
nix develop

# Or with direnv (auto-loads)
direnv allow

# Use just commands
just build    # Build everything
just test     # Run tests
just watch    # Watch mode with bacon
just fmt      # Format code
```

### Testing
```bash
# Run all tests
just test

# With coverage
just coverage

# Benchmarks
just bench

# Memory debugging
just valgrind
```

### Release Process
```bash
# Create release (updates version everywhere)
just release v0.1.0

# Build for all platforms
just build-all

# Push to Cachix cache
just cache-push
```

### Cross-Platform Builds
```bash
# Linux (static)
nix build .#landropic-static

# ARM64 Linux
nix build .#landropic-aarch64-linux

# Docker
nix build .#docker
docker load < result
```

## 🔧 Advanced Usage

### NixOS Deployment

Add to your NixOS configuration:
```nix
{
  imports = [ 
    (builtins.fetchGit {
      url = "https://github.com/rexbrahh/landropic";
      ref = "main";
    }).nixosModules.landropic
  ];

  services.landropic = {
    enable = true;
    syncDirs = [ "/home/user/Documents" ];
    openFirewall = true;
  };
}
```

### Home Manager

Add to your Home Manager configuration:
```nix
{
  imports = [ 
    landropic.homeManagerModules.landropic
  ];

  services.landropic = {
    enable = true;
    syncDirs = [ "Documents" "Pictures" ];
  };
}
```

### Custom Shell

Create a custom shell for your workflow:
```nix
# shell.nix
{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  inputsFrom = [ 
    (builtins.getFlake "github:rexbrahh/landropic").devShells.${pkgs.system}.default
  ];
  
  buildInputs = with pkgs; [
    # Add your tools here
  ];
}
```

## 🚄 CI/CD Integration

### GitHub Actions

The project uses Nix for all CI:
- Automatic caching with Magic Nix Cache
- Cachix binary cache for fast builds
- Multi-platform releases
- Docker image publishing

### Local CI Simulation
```bash
# Run same checks as CI
just ci

# Check flake
nix flake check

# Build all checks
nix build .#checks.x86_64-linux.all
```

## 🔍 Troubleshooting

### Common Issues

1. **"experimental-features" error**
   ```bash
   echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
   ```

2. **Out of disk space**
   ```bash
   # Garbage collect old builds
   nix-collect-garbage -d
   
   # Or keep last 7 days
   nix-collect-garbage --delete-older-than 7d
   ```

3. **Slow first build**
   - First build downloads all dependencies
   - Subsequent builds use cache
   - Consider using Cachix for binary cache

4. **Platform-specific issues**
   ```bash
   # Force rebuild
   nix build --rebuild
   
   # Check what will be built
   nix build --dry-run
   ```

### Debugging Nix

```bash
# Show derivation
nix show-derivation .#landropic

# Build with verbose output
nix build -L -v

# Enter build environment
nix develop --command bash

# Show dependency tree
nix-tree

# Compare two derivations
nix-diff /nix/store/hash1 /nix/store/hash2
```

## 📊 Performance Tips

1. **Use Cachix**: Pre-built binaries for common dependencies
2. **Enable parallel builds**: Already configured in flake
3. **Use direnv**: Automatic environment loading
4. **Incremental builds**: Crane handles this automatically

## 🔐 Security

Nix provides:
- **Reproducible builds**: Same input → same output
- **Supply chain security**: All dependencies pinned
- **Sandboxed builds**: Isolated from host system
- **Signed packages**: Cachix signs all binaries

## 📚 Resources

- [Nix Manual](https://nixos.org/manual/nix/stable/)
- [Nix Pills](https://nixos.org/guides/nix-pills/)
- [Zero to Nix](https://zero-to-nix.com/)
- [Crane Documentation](https://crane.dev/)
- [Cachix Documentation](https://docs.cachix.org/)