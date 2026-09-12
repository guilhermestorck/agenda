#!/usr/bin/env bash
# Install agenda for the current user, under ~/.local. No root, nothing outside $HOME.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
prefix="${PREFIX:-$HOME/.local}"
app_id="io.github.guilhermestorck.agenda"

echo "Building (release)..."
cargo build --release --manifest-path "$here/Cargo.toml"

install -Dm755 "$here/target/release/agenda" "$prefix/bin/agenda"
install -Dm644 "$here/data/$app_id.desktop" "$prefix/share/applications/$app_id.desktop"
install -Dm644 "$here/data/$app_id.svg" \
    "$prefix/share/icons/hicolor/scalable/apps/$app_id.svg"

# Only refreshes the launcher's view of what is installed; harmless when absent.
command -v update-desktop-database >/dev/null && \
    update-desktop-database "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && \
    gtk-update-icon-cache -qtf "$prefix/share/icons/hicolor" 2>/dev/null || true

echo "Installed to $prefix/bin/agenda"
case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "Note: $prefix/bin is not on your PATH." ;;
esac

# Autostart is opt-in. A calendar that adds itself to your session uninvited is a calendar
# you uninstall.
cat <<HINT

To start agenda with your session:
    mkdir -p ~/.config/autostart
    cp "$prefix/share/applications/$app_id.desktop" ~/.config/autostart/

To remove it:
    rm -f "$prefix/bin/agenda" \\
          "$prefix/share/applications/$app_id.desktop" \\
          "$prefix/share/icons/hicolor/scalable/apps/$app_id.svg" \\
          ~/.config/autostart/$app_id.desktop
HINT
