#!/bin/bash
# Installs the Olorunns plugins on this Mac so Ableton Live can find them.
#
# Run it from Terminal with:   bash ~/Downloads/Olorunns-Plugins-macOS/install.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="$HOME/Library/Audio/Plug-Ins/VST3"
mkdir -p "$dest"

shopt -s nullglob
plugins=("$here"/*.vst3)
if [ ${#plugins[@]} -eq 0 ]; then
    echo "Couldn't find any .vst3 plugins next to this script." >&2
    exit 1
fi

for plugin in "${plugins[@]}"; do
    name="$(basename "$plugin")"
    rm -rf "${dest:?}/$name"
    cp -R "$plugin" "$dest/"
    # macOS marks downloaded files as quarantined and refuses to load them in Ableton.
    # These plugins aren't notarized by Apple, so clear that mark.
    xattr -dr com.apple.quarantine "$dest/$name" 2>/dev/null || true
    echo "Installed ${name%.vst3}"
done

echo
echo "All done! Now in Ableton Live:"
echo "  1. Open Settings > Plug-Ins"
echo "  2. Turn on 'Use VST3 Plug-In System Folders'"
echo "  3. Click 'Rescan'"
echo "The plugins appear in the browser under Plug-Ins > Olorunns."
