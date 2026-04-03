#!/usr/bin/env bash
# Boot a QEMU VM and run the install script inside it.
#
# Supports Ubuntu 24.04 and Amazon Linux 2023, both aarch64 and x86_64.
# On Apple Silicon the aarch64 VM uses HVF acceleration (near-native speed).
#
# Usage:
#   ./scripts/test-install-vm.sh                     # Ubuntu aarch64 (default)
#   ./scripts/test-install-vm.sh --os amazonlinux     # Amazon Linux 2023
#   ./scripts/test-install-vm.sh --arch x86_64        # x86_64 (emulated, slow)
#   ./scripts/test-install-vm.sh --keep               # don't destroy VM after test
#
# Prerequisites: qemu (brew install qemu)
#
# The script:
#   1. Downloads a cloud image (cached in .qemu-cache/)
#   2. Creates a cloud-init ISO to inject SSH keys + the install script
#   3. Boots the VM with port-forwarded SSH (host 2222 → guest 22)
#   4. Waits for SSH, copies install.sh, runs it non-interactively
#   5. Verifies the install succeeded (binary exists, service running)
#   6. Tears down the VM

set -euo pipefail

# ─── Defaults ─────────────────────────────────────────────────────────────────
GUEST_OS="ubuntu"
GUEST_ARCH="aarch64"
KEEP_VM=0
VM_RAM="4G"
VM_CPUS="4"
VM_DISK_SIZE="20G"
SSH_PORT="2222"
SSH_TIMEOUT=180       # seconds to wait for SSH

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
CACHE_DIR="${ROOT_DIR}/.qemu-cache"
WORK_DIR="${ROOT_DIR}/.qemu-work"

# ─── Parse args ───────────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --os)         GUEST_OS="$2"; shift 2 ;;
        --arch)       GUEST_ARCH="$2"; shift 2 ;;
        --keep)       KEEP_VM=1; shift ;;
        --ram)        VM_RAM="$2"; shift 2 ;;
        --cpus)       VM_CPUS="$2"; shift 2 ;;
        --ssh-port)   SSH_PORT="$2"; shift 2 ;;
        --help|-h)
            sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ─── Colours ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m'

info()  { printf "${GREEN}[+]${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}[!]${NC} %s\n" "$*"; }
err()   { printf "${RED}[x]${NC} %s\n" "$*" >&2; }
step()  { printf "\n${BOLD}${BLUE}==> %s${NC}\n" "$*"; }

# ─── Cleanup ──────────────────────────────────────────────────────────────────
QEMU_PID=""

cleanup() {
    if [ -n "$QEMU_PID" ] && kill -0 "$QEMU_PID" 2>/dev/null; then
        if [ "$KEEP_VM" = "1" ]; then
            warn "VM still running (PID $QEMU_PID, SSH: ssh -p $SSH_PORT runtara@localhost)"
        else
            info "Shutting down VM (PID $QEMU_PID)"
            kill "$QEMU_PID" 2>/dev/null || true
            wait "$QEMU_PID" 2>/dev/null || true
        fi
    fi
    if [ "$KEEP_VM" = "0" ] && [ -d "$WORK_DIR" ]; then
        rm -rf "$WORK_DIR"
    fi
}
trap cleanup EXIT

# ─── Resolve image URLs ──────────────────────────────────────────────────────

resolve_image() {
    case "${GUEST_OS}" in
        ubuntu)
            case "${GUEST_ARCH}" in
                aarch64)
                    IMAGE_URL="https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-arm64.img"
                    IMAGE_FILE="ubuntu-24.04-arm64.img"
                    ;;
                x86_64)
                    IMAGE_URL="https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img"
                    IMAGE_FILE="ubuntu-24.04-amd64.img"
                    ;;
            esac
            ;;
        amazonlinux|al2023)
            case "${GUEST_ARCH}" in
                aarch64)
                    IMAGE_URL="https://cdn.amazonlinux.com/al2023/os-images/2023.7.20250331/kvm-arm64/al2023-kvm-2023.7.20250331-kernel-6.1-arm64.xfs.gpt.qcow2"
                    IMAGE_FILE="al2023-arm64.qcow2"
                    ;;
                x86_64)
                    IMAGE_URL="https://cdn.amazonlinux.com/al2023/os-images/2023.7.20250331/kvm/al2023-kvm-2023.7.20250331-kernel-6.1-x86_64.xfs.gpt.qcow2"
                    IMAGE_FILE="al2023-x86_64.qcow2"
                    ;;
            esac
            ;;
        *)
            err "Unsupported OS: ${GUEST_OS}. Use 'ubuntu' or 'amazonlinux'."
            exit 1
            ;;
    esac
}

# ─── Download image ──────────────────────────────────────────────────────────

download_image() {
    step "Preparing VM image"
    mkdir -p "$CACHE_DIR"

    if [ -f "$CACHE_DIR/$IMAGE_FILE" ]; then
        info "Using cached image: $IMAGE_FILE"
    else
        info "Downloading $IMAGE_FILE (this is a one-time download)"
        curl -fSL -o "$CACHE_DIR/$IMAGE_FILE" "$IMAGE_URL"
    fi

    # Create a working copy so we don't mutate the cached base
    mkdir -p "$WORK_DIR"
    cp "$CACHE_DIR/$IMAGE_FILE" "$WORK_DIR/disk.qcow2"

    # Resize disk to ensure enough space for Rust + builds
    qemu-img resize "$WORK_DIR/disk.qcow2" "$VM_DISK_SIZE"
    info "Disk resized to $VM_DISK_SIZE"
}

# ─── Cloud-init ──────────────────────────────────────────────────────────────

create_cloud_init() {
    step "Creating cloud-init seed"
    local ci_dir="$WORK_DIR/cloud-init"
    mkdir -p "$ci_dir"

    # Generate a temporary SSH key for this test
    ssh-keygen -t ed25519 -f "$WORK_DIR/test_key" -N "" -q
    local pubkey
    pubkey="$(cat "$WORK_DIR/test_key.pub")"

    cat > "$ci_dir/meta-data" <<EOF
instance-id: runtara-install-test
local-hostname: runtara-test
EOF

    cat > "$ci_dir/user-data" <<EOF
#cloud-config
users:
  - name: runtara
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - ${pubkey}

# Grow the root partition to fill the disk
growpart:
  mode: auto
  devices: ['/']
resize_rootfs: true

# Install PostgreSQL + Valkey so the install script's connectivity checks pass
package_update: true
packages:
  - postgresql
  - curl

runcmd:
  # Start PostgreSQL and configure for local trust auth (test VM only)
  - systemctl enable --now postgresql
  - |
    PG_HBA=\$(find /etc/postgresql -name pg_hba.conf 2>/dev/null | head -1)
    if [ -n "\$PG_HBA" ]; then
      # Replace any auth method on local TCP lines with trust
      sed -i -E 's/^(host\s+all\s+all\s+127\.0\.0\.1\/32\s+).*/\1trust/' "\$PG_HBA"
      sed -i -E 's/^(host\s+all\s+all\s+::1\/128\s+).*/\1trust/' "\$PG_HBA"
      systemctl reload postgresql
    fi
  - su - postgres -c "createuser --superuser runtara" || true
  - su - postgres -c "createdb -O runtara runtara" || true
  - su - postgres -c "createdb -O runtara runtara_objects" || true

  # Install Valkey (or Redis as fallback)
  - |
    if command -v dnf >/dev/null 2>&1; then
      dnf install -y redis || true
      systemctl enable --now redis
    else
      apt-get install -y -qq redis-server || true
      systemctl enable --now redis-server
    fi

  # Signal that cloud-init is done
  - touch /tmp/cloud-init-done
EOF

    # Create the cloud-init ISO (NoCloud datasource)
    if command -v mkisofs > /dev/null 2>&1; then
        mkisofs -output "$WORK_DIR/seed.iso" -volid cidata -joliet -rock \
            "$ci_dir/user-data" "$ci_dir/meta-data" 2>/dev/null
    elif command -v hdiutil > /dev/null 2>&1; then
        # macOS fallback: create a FAT disk image
        dd if=/dev/zero of="$WORK_DIR/seed.img" bs=1M count=1 2>/dev/null
        DISK_DEV="$(hdiutil attach -nomount "$WORK_DIR/seed.img" | head -1 | awk '{print $1}')"
        diskutil eraseDisk MS-DOS CIDATA MBR "$DISK_DEV" > /dev/null
        MOUNT_POINT="$(diskutil info "$DISK_DEV"s1 | grep "Mount Point" | awk '{print $NF}')"
        if [ -z "$MOUNT_POINT" ]; then
            # Mount it explicitly
            mkdir -p "$WORK_DIR/mnt"
            mount -t msdos "${DISK_DEV}s1" "$WORK_DIR/mnt"
            MOUNT_POINT="$WORK_DIR/mnt"
        fi
        cp "$ci_dir/user-data" "$MOUNT_POINT/"
        cp "$ci_dir/meta-data" "$MOUNT_POINT/"
        hdiutil detach "$DISK_DEV" > /dev/null
        mv "$WORK_DIR/seed.img" "$WORK_DIR/seed.iso"
    else
        # Use genisoimage / xorriso if available
        genisoimage -output "$WORK_DIR/seed.iso" -volid cidata -joliet -rock \
            "$ci_dir/user-data" "$ci_dir/meta-data" 2>/dev/null \
            || { err "No ISO creation tool found. Install cdrtools: brew install cdrtools"; exit 1; }
    fi

    info "Cloud-init seed created"
}

# ─── UEFI firmware ────────────────────────────────────────────────────────────

resolve_firmware() {
    case "${GUEST_ARCH}" in
        aarch64)
            # aarch64 requires UEFI firmware
            QEMU_BIN="qemu-system-aarch64"
            MACHINE_TYPE="virt"
            ACCEL_FLAG=""

            # Check for HVF on macOS arm64 host
            if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
                ACCEL_FLAG="-accel hvf"
                MACHINE_TYPE="virt,highmem=on"
            fi

            # Find UEFI firmware
            FIRMWARE=""
            for f in \
                "/opt/homebrew/share/qemu/edk2-aarch64-code.fd" \
                "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd" \
                "/usr/share/AAVMF/AAVMF_CODE.fd"; do
                if [ -f "$f" ]; then
                    FIRMWARE="$f"
                    break
                fi
            done
            if [ -z "$FIRMWARE" ]; then
                err "UEFI firmware for aarch64 not found. Install: brew install qemu"
                exit 1
            fi

            FIRMWARE_ARGS="-bios $FIRMWARE"
            CPU_TYPE="host"
            if [ -z "$ACCEL_FLAG" ]; then
                CPU_TYPE="cortex-a72"
            fi
            ;;
        x86_64)
            QEMU_BIN="qemu-system-x86_64"
            MACHINE_TYPE="q35"
            ACCEL_FLAG=""
            FIRMWARE_ARGS=""
            CPU_TYPE="qemu64"

            # KVM on Linux x86_64
            if [ "$(uname -s)" = "Linux" ] && [ -w /dev/kvm ]; then
                ACCEL_FLAG="-accel kvm"
                CPU_TYPE="host"
            fi
            ;;
    esac
}

# ─── Boot VM ─────────────────────────────────────────────────────────────────

boot_vm() {
    step "Booting ${GUEST_OS} ${GUEST_ARCH} VM"

    local seed_drive=""
    if [ -f "$WORK_DIR/seed.iso" ]; then
        seed_drive="-drive file=$WORK_DIR/seed.iso,format=raw,if=virtio,media=cdrom"
    fi

    # shellcheck disable=SC2086
    $QEMU_BIN \
        -machine "$MACHINE_TYPE" \
        $ACCEL_FLAG \
        -cpu "$CPU_TYPE" \
        -m "$VM_RAM" \
        -smp "$VM_CPUS" \
        $FIRMWARE_ARGS \
        -drive file="$WORK_DIR/disk.qcow2",format=qcow2,if=virtio \
        $seed_drive \
        -netdev user,id=net0,hostfwd=tcp::${SSH_PORT}-:22 \
        -device virtio-net-pci,netdev=net0 \
        -nographic \
        -serial mon:stdio \
        > "$WORK_DIR/qemu.log" 2>&1 &

    QEMU_PID=$!
    info "VM booting (PID: $QEMU_PID), waiting for SSH on port $SSH_PORT"
}

# ─── SSH helpers ──────────────────────────────────────────────────────────────
# scp uses -P (capital) for port; ssh uses -p (lowercase).

vm_ssh() {
    ssh -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null \
        -o ConnectTimeout=5 \
        -i "$WORK_DIR/test_key" \
        -p "$SSH_PORT" \
        runtara@localhost "$@"
}

vm_scp() {
    scp -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null \
        -i "$WORK_DIR/test_key" \
        -P "$SSH_PORT" \
        "$@"
}

# ─── Wait for SSH ─────────────────────────────────────────────────────────────

wait_for_ssh() {
    local elapsed=0

    while [ $elapsed -lt $SSH_TIMEOUT ]; do
        if vm_ssh "test -f /tmp/cloud-init-done" 2>/dev/null; then
            info "SSH ready, cloud-init complete (${elapsed}s)"
            return 0
        fi
        sleep 5
        elapsed=$((elapsed + 5))
        printf "."
    done

    echo ""
    err "SSH not available after ${SSH_TIMEOUT}s. VM log:"
    tail -30 "$WORK_DIR/qemu.log"
    exit 1
}

# ─── Run install ──────────────────────────────────────────────────────────────

run_install() {
    step "Running install script inside VM"

    # Copy the install script to the VM
    vm_scp "$ROOT_DIR/scripts/install.sh" runtara@localhost:/tmp/install.sh

    # Create a stub runtara-server binary inside the VM.
    info "Creating stub runtara-server binary in VM"
    vm_ssh "printf '#!/bin/sh\necho runtara-server stub\n' > /tmp/runtara-server && chmod +x /tmp/runtara-server"

    # Copy repo source for library cache build.
    # The latest release tag may not yet have wasm32-wasip2 support, so we use
    # the current working tree which has the correct feature-gated stdlib.
    info "Copying repo source to VM (for stdlib library build)"
    vm_ssh "mkdir -p /tmp/runtara-src"
    (cd "$ROOT_DIR" && git archive HEAD | gzip) | vm_ssh "tar -xz -C /tmp/runtara-src"

    # Run the install script non-interactively.
    vm_ssh sudo env \
        RUNTARA_NONINTERACTIVE=1 \
        RUNTARA_BINARY_PATH="/tmp/runtara-server" \
        RUNTARA_SOURCE_DIR="/tmp/runtara-src" \
        RUNTARA_DATABASE_URL="postgres://runtara@localhost/runtara" \
        OBJECT_MODEL_DATABASE_URL="postgres://runtara@localhost/runtara_objects" \
        VALKEY_HOST="127.0.0.1" \
        VALKEY_PORT="6379" \
        OAUTH2_JWKS_URI="https://example.com/.well-known/jwks.json" \
        OAUTH2_ISSUER="https://example.com" \
        TENANT_ID="test_tenant" \
        SERVER_PORT="7001" \
        bash -x /tmp/install.sh
}

# ─── Verify install ──────────────────────────────────────────────────────────

verify_install() {
    step "Verifying installation"

    local failures=0

    check() {
        local desc="$1"; shift
        if vm_ssh "$@" 2>/dev/null; then
            info "PASS: $desc"
        else
            err "FAIL: $desc"
            failures=$((failures + 1))
        fi
    }

    check "runtara-server binary exists"      "test -x /usr/local/bin/runtara-server"
    check "runtara-server runs"               "/usr/local/bin/runtara-server"
    check "wasmtime binary exists"            "test -x /usr/local/bin/wasmtime"
    check "config file exists"                "test -f /etc/runtara/runtara-server.conf"
    check "config has TENANT_ID"              "grep -q TENANT_ID /etc/runtara/runtara-server.conf"
    check "config has VALKEY_HOST"            "grep -q VALKEY_HOST /etc/runtara/runtara-server.conf"
    check "config has OAUTH2_JWKS_URI"        "grep -q OAUTH2_JWKS_URI /etc/runtara/runtara-server.conf"
    check "stdlib rlib exists"                "test -f /usr/share/runtara/library_cache/wasm/libruntara_workflow_stdlib.rlib"
    check "library cache has deps"            "test -d /usr/share/runtara/library_cache/wasm/deps"
    check "library cache has dep rlibs"       "ls /usr/share/runtara/library_cache/wasm/deps/*.rlib >/dev/null 2>&1"
    check "runtara user exists"               "id runtara"
    check "data directory exists"             "test -d /var/lib/runtara"
    check "data dir owned by runtara"         "test \$(stat -c %U /var/lib/runtara) = runtara"
    check "config permissions restricted"     "test \$(stat -c %a /etc/runtara/runtara-server.conf) = 640"
    check "systemd unit installed"            "systemctl is-enabled runtara-server"
    check "rustc is available"                "bash -lc 'rustc --version'"
    check "wasm32-wasip2 target installed"    "bash -lc 'rustup target list --installed | grep wasm32-wasip2'"

    echo ""
    if [ "$failures" -eq 0 ]; then
        printf '%s  All checks passed!%s\n' "${GREEN}${BOLD}" "$NC"
    else
        printf '%s  %d check(s) failed%s\n' "${RED}${BOLD}" "$failures" "$NC"
        exit 1
    fi
}

# ─── Main ─────────────────────────────────────────────────────────────────────

main() {
    printf '\n%s  Runtara Install Test (QEMU VM)%s\n' "${BOLD}" "$NC"
    echo "  OS: ${GUEST_OS}  Arch: ${GUEST_ARCH}  RAM: ${VM_RAM}  CPUs: ${VM_CPUS}"
    echo ""

    resolve_image
    download_image
    create_cloud_init
    resolve_firmware
    boot_vm
    wait_for_ssh
    run_install
    verify_install
}

main "$@"
