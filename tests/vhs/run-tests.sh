#!/bin/bash
#
# VHS Test Runner for claude-chill
#
# Usage: ./tests/vhs/run-tests.sh [--install-vhs] [tape_file...]
#
# If no tape files specified, runs all .tape files in tests/vhs/
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="$SCRIPT_DIR/output"
VHS_VERSION="0.10.0"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Parse arguments
INSTALL_VHS=false
TAPE_FILES=()

for arg in "$@"; do
    case $arg in
        --install-vhs)
            INSTALL_VHS=true
            ;;
        *)
            TAPE_FILES+=("$arg")
            ;;
    esac
done

# Function to install VHS
install_vhs() {
    echo -e "${YELLOW}Installing VHS v${VHS_VERSION}...${NC}"

    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)
            if [ "$ARCH" = "x86_64" ]; then
                VHS_FILE="vhs_${VHS_VERSION}_amd64.deb"
            elif [ "$ARCH" = "aarch64" ]; then
                VHS_FILE="vhs_${VHS_VERSION}_arm64.deb"
            else
                echo -e "${RED}Unsupported architecture: $ARCH${NC}"
                exit 1
            fi

            curl -fsSL -o "/tmp/$VHS_FILE" \
                "https://github.com/charmbracelet/vhs/releases/download/v${VHS_VERSION}/${VHS_FILE}"

            if command -v sudo &> /dev/null; then
                sudo dpkg -i "/tmp/$VHS_FILE" || sudo apt-get install -f -y
            else
                dpkg -i "/tmp/$VHS_FILE" || apt-get install -f -y
            fi
            rm -f "/tmp/$VHS_FILE"
            ;;
        Darwin)
            if command -v brew &> /dev/null; then
                brew install charmbracelet/tap/vhs
            else
                echo -e "${RED}Homebrew not found. Please install VHS manually.${NC}"
                exit 1
            fi
            ;;
        *)
            echo -e "${RED}Unsupported OS: $OS${NC}"
            exit 1
            ;;
    esac

    # VHS requires ttyd and ffmpeg
    if [ "$OS" = "Linux" ]; then
        if command -v sudo &> /dev/null; then
            sudo apt-get update && sudo apt-get install -y ttyd ffmpeg
        else
            apt-get update && apt-get install -y ttyd ffmpeg
        fi
    elif [ "$OS" = "Darwin" ]; then
        brew install ttyd ffmpeg
    fi

    echo -e "${GREEN}VHS installed successfully!${NC}"
}

# Check for VHS installation
check_vhs() {
    if ! command -v vhs &> /dev/null; then
        echo -e "${RED}VHS is not installed.${NC}"
        echo "Run with --install-vhs to install, or install manually:"
        echo "  Linux: See https://github.com/charmbracelet/vhs#installation"
        echo "  macOS: brew install charmbracelet/tap/vhs"
        exit 1
    fi
    echo -e "${GREEN}VHS found: $(vhs --version)${NC}"
}

# Install VHS if requested
if [ "$INSTALL_VHS" = true ]; then
    install_vhs
fi

# Check VHS is available
check_vhs

# Build the project first
echo -e "${YELLOW}Building claude-chill...${NC}"
cd "$PROJECT_ROOT"
cargo build

# Create output directory
mkdir -p "$OUTPUT_DIR"

# If no tape files specified, find all .tape files
if [ ${#TAPE_FILES[@]} -eq 0 ]; then
    mapfile -t TAPE_FILES < <(find "$SCRIPT_DIR" -maxdepth 1 -name "*.tape" -type f | sort)
fi

# Run each tape file
PASSED=0
FAILED=0
SKIPPED=0

echo ""
echo -e "${YELLOW}Running VHS tests...${NC}"
echo "========================================"

for tape in "${TAPE_FILES[@]}"; do
    tape_name=$(basename "$tape" .tape)
    echo -n "Running $tape_name... "

    # Run VHS
    if vhs "$tape" 2>&1 | tee "$OUTPUT_DIR/${tape_name}.log" | grep -q "Error"; then
        echo -e "${RED}FAILED${NC}"
        ((FAILED++))
    else
        echo -e "${GREEN}PASSED${NC}"
        ((PASSED++))
    fi
done

echo "========================================"
echo ""
echo -e "Results: ${GREEN}$PASSED passed${NC}, ${RED}$FAILED failed${NC}, ${YELLOW}$SKIPPED skipped${NC}"
echo ""
echo "Output files saved to: $OUTPUT_DIR/"

# Exit with error if any tests failed
if [ $FAILED -gt 0 ]; then
    exit 1
fi
