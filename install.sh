#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
cargo build --release
install -Dm755 target/release/tinyhand-linux "${HOME}/.local/bin/tinyhand-linux"
install -Dm644 assets/hand_pastel.png "${HOME}/.local/share/tinyhand-linux/hand_pastel.png"
install -Dm644 assets/tap_plastic.wav "${HOME}/.local/share/tinyhand-linux/tap_plastic.wav"
printf 'Installed %s\n' "${HOME}/.local/bin/tinyhand-linux"
printf 'Run: tinyhand-linux\n'
