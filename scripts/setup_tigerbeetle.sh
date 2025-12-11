#!/bin/bash
# ==============================================================================
# TigerBeetle Setup Script
# Formats the data file required before starting TigerBeetle
# ==============================================================================

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DATA_DIR="$PROJECT_DIR/data/tigerbeetle"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Ensure data directory exists
mkdir -p "$DATA_DIR"

case "${1:-single}" in
    single)
        log_info "Setting up TigerBeetle single-node (development)"

        if [ -f "$DATA_DIR/0_0.tigerbeetle" ]; then
            log_warn "Data file already exists: $DATA_DIR/0_0.tigerbeetle"
            read -p "Do you want to recreate it? This will DELETE all data. (y/N): " confirm
            if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
                log_info "Keeping existing data file."
                exit 0
            fi
            rm -f "$DATA_DIR/0_0.tigerbeetle"
        fi

        log_info "Formatting TigerBeetle data file..."
        docker run --rm \
            --security-opt seccomp=unconfined \
            -v "$DATA_DIR:/data" \
            ghcr.io/tigerbeetle/tigerbeetle \
            format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle

        log_info "✅ TigerBeetle data file created: $DATA_DIR/0_0.tigerbeetle"
        log_info ""
        log_info "To start TigerBeetle:"
        log_info "  docker-compose -f docker-compose.tigerbeetle.yml up -d"
        ;;

    cluster)
        log_info "Setting up TigerBeetle 3-node cluster (production)"

        for i in 0 1 2; do
            if [ -f "$DATA_DIR/0_$i.tigerbeetle" ]; then
                log_warn "Data file already exists: $DATA_DIR/0_$i.tigerbeetle"
                rm -f "$DATA_DIR/0_$i.tigerbeetle"
            fi

            log_info "Formatting replica $i..."
            docker run --rm \
                --security-opt seccomp=unconfined \
                -v "$DATA_DIR:/data" \
                ghcr.io/tigerbeetle/tigerbeetle \
                format --cluster=0 --replica=$i --replica-count=3 /data/0_$i.tigerbeetle
        done

        log_info "✅ TigerBeetle cluster data files created"
        log_info ""
        log_info "To start the cluster, uncomment the cluster section in docker-compose.tigerbeetle.yml"
        ;;

    clean)
        log_warn "Cleaning TigerBeetle data..."
        rm -rf "$DATA_DIR"
        log_info "✅ TigerBeetle data cleaned"
        ;;

    status)
        log_info "TigerBeetle Status:"
        if docker ps | grep -q tigerbeetle; then
            docker ps --filter "name=tigerbeetle" --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}"
        else
            log_warn "TigerBeetle is not running"
        fi

        log_info ""
        log_info "Data files:"
        ls -lh "$DATA_DIR"/*.tigerbeetle 2>/dev/null || log_warn "No data files found"
        ;;

    *)
        echo "Usage: $0 {single|cluster|clean|status}"
        echo ""
        echo "Commands:"
        echo "  single  - Format for single-node development setup (default)"
        echo "  cluster - Format for 3-node production cluster"
        echo "  clean   - Remove all TigerBeetle data"
        echo "  status  - Show TigerBeetle status and data files"
        exit 1
        ;;
esac
