#!/bin/bash
set -e

# Pre-commit setup script for Landropic
# Senior Engineer Build Validation Infrastructure

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if we're in a git repository
if ! git rev-parse --git-dir > /dev/null 2>&1; then
    log_error "This script must be run from within a git repository"
    exit 1
fi

log_info "Setting up pre-commit hooks for Landropic..."

# Check if Python is available
if ! command -v python3 &> /dev/null && ! command -v python &> /dev/null; then
    log_error "Python is required for pre-commit hooks"
    log_info "Please install Python 3: https://www.python.org/downloads/"
    exit 1
fi

# Determine Python command
PYTHON_CMD="python3"
if ! command -v python3 &> /dev/null; then
    PYTHON_CMD="python"
fi

log_info "Using Python: $($PYTHON_CMD --version)"

# Check if pip is available
if ! $PYTHON_CMD -m pip --version &> /dev/null; then
    log_error "pip is required for installing pre-commit"
    log_info "Please install pip or ensure it's available in your Python installation"
    exit 1
fi

# Install pre-commit if not already installed
if ! command -v pre-commit &> /dev/null; then
    log_info "Installing pre-commit..."
    $PYTHON_CMD -m pip install pre-commit
    log_success "pre-commit installed successfully"
else
    log_info "pre-commit is already installed: $(pre-commit --version)"
fi

# Verify pre-commit configuration exists
if [[ ! -f .pre-commit-config.yaml ]]; then
    log_error "Pre-commit configuration file (.pre-commit-config.yaml) not found"
    log_info "Please ensure the configuration file exists in the repository root"
    exit 1
fi

# Install the pre-commit hooks
log_info "Installing pre-commit hooks..."
if pre-commit install; then
    log_success "Pre-commit hooks installed successfully"
else
    log_error "Failed to install pre-commit hooks"
    exit 1
fi

# Install pre-push hooks
log_info "Installing pre-push hooks..."
if pre-commit install --hook-type pre-push; then
    log_success "Pre-push hooks installed successfully"
else
    log_warning "Failed to install pre-push hooks (continuing anyway)"
fi

# Install commit-msg hooks
log_info "Installing commit-msg hooks..."
if pre-commit install --hook-type commit-msg; then
    log_success "Commit-msg hooks installed successfully"
else
    log_warning "Failed to install commit-msg hooks (continuing anyway)"
fi

# Verify Rust toolchain is available
log_info "Verifying Rust toolchain..."
if ! command -v cargo &> /dev/null; then
    log_warning "Cargo not found. Some pre-commit hooks may fail."
    log_info "Please install Rust: https://rustup.rs/"
else
    log_success "Cargo found: $(cargo --version)"
fi

if ! command -v rustfmt &> /dev/null; then
    log_warning "rustfmt not found. Installing rustfmt component..."
    rustup component add rustfmt || log_warning "Failed to install rustfmt"
else
    log_success "rustfmt available"
fi

if ! command -v clippy-driver &> /dev/null; then
    log_warning "clippy not found. Installing clippy component..."
    rustup component add clippy || log_warning "Failed to install clippy"
else
    log_success "clippy available"
fi

# Optional: Run hooks on all files to verify setup
read -p "Would you like to run pre-commit on all files to verify setup? (y/N): " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    log_info "Running pre-commit on all files..."
    if pre-commit run --all-files; then
        log_success "All pre-commit hooks passed!"
    else
        log_warning "Some pre-commit hooks failed. This is normal for initial setup."
        log_info "The hooks will automatically run on your next commit."
    fi
fi

echo
log_success "Pre-commit setup completed!"
echo
echo "Next steps:"
echo "  1. Pre-commit hooks will now run automatically on every commit"
echo "  2. Use 'git commit --no-verify' to skip hooks if absolutely necessary"
echo "  3. Use 'pre-commit run --all-files' to run hooks manually on all files"
echo "  4. Use 'pre-commit autoupdate' to update hook versions"
echo
echo "Hook configuration:"
echo "  • Format check (cargo fmt)"
echo "  • Linting (cargo clippy)"
echo "  • Build check (cargo check)"
echo "  • Quick tests (on pre-push)"
echo "  • Security audit (on pre-push)"
echo "  • File format validation (YAML, TOML, JSON)"
echo "  • Basic security checks"
echo
log_info "Happy coding! Your commits are now protected against build failures."