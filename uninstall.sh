#!/bin/bash

echo "🗑️  Uninstalling rtimer from KDE Plasma..."
echo ""

# Check if we're in a KDE environment
if [ -z "$KDE_SESSION_VERSION" ] && [ -z "$XDG_CURRENT_DESKTOP" ]; then
    echo "⚠️  Warning: Not running in KDE Plasma environment"
    echo "   Some cleanup steps may be skipped."
    echo ""
fi

# Remove binary
echo "1. Removing binary..."
if sudo rm -f /usr/local/bin/rtimer; then
    echo "   ✓ Binary removed"
else
    echo "   ✗ Could not remove binary"
fi

# Remove desktop entry
echo ""
echo "2. Removing desktop entry..."
if rm -f "$HOME/.local/share/applications/rtimer.desktop"; then
    echo "   ✓ Desktop entry removed"
else
    echo "   ✗ Could not remove desktop entry"
fi

# Remove icon
echo ""
echo "3. Removing icon..."
if rm -f "$HOME/.local/share/icons/hicolor/scalable/apps/rtimer.svg"; then
    echo "   ✓ Icon removed"
else
    echo "   ✗ Could not remove icon"
fi

# Ask about config/stats
echo ""
read -p "4. Remove statistics and configuration? (y/N) " -n 1 -r
echo ""
if [[ $REPLY =~ ^[Yy]$ ]]; then
    if rm -rf "$HOME/.config/rtimer" 2>/dev/null; then
        echo "   ✓ Configuration removed"
    else
        echo "   ✗ Could not remove configuration"
    fi
fi

# Clear KDE menu cache
echo ""
echo "5. Clearing KDE menu cache..."
# Multiple possible cache locations
cache_locations=(
    "$HOME/.cache/plasmashell"
    "$HOME/.cache/krunner"
    "$HOME/.cache/org.kde.plasma.notifications"
)

for cache in "${cache_locations[@]}"; do
    if [ -d "$cache" ]; then
        rm -rf "$cache"/*.cache 2>/dev/null
        rm -rf "$cache"/*.kcache 2>/dev/null
    fi
done
echo "   ✓ KDE caches cleared"

# Rebuild KDE configuration
echo ""
echo "6. Rebuilding KDE configuration..."

# Try different KDE versions
for cmd in kbuildsycoca6 kbuildsycoca5 kbuildsycoca4; do
    if command -v "$cmd" &> /dev/null; then
        echo "   Running $cmd..."
        "$cmd" 2>/dev/null || true
    fi
done

# Update icon caches
echo ""
echo "7. Updating icon caches..."
if command -v kiconinst5 &> /dev/null; then
    kiconinst5 --quiet 2>/dev/null || true
    echo "   ✓ KDE icon cache updated"
fi

if command -v gtk-update-icon-cache &> /dev/null; then
    gtk-update-icon-cache ~/.local/share/icons/hicolor -f 2>/dev/null || true
    echo "   ✓ GTK icon cache updated"
fi

# Restart Plasma if possible
echo ""
read -p "8. Restart Plasma shell to apply changes? (y/N) " -n 1 -r
echo ""
if [[ $REPLY =~ ^[Yy]$ ]]; then
    if command -v plasmashell &> /dev/null; then
        echo "   Restarting Plasma shell..."
        killall plasmashell 2>/dev/null || true
        sleep 1
        plasmashell --replace & disown
        echo "   ✓ Plasma shell restarted"
    else
        echo "   ✗ plasmashell command not found"
    fi
fi

echo ""
echo "🎉 rtimer has been completely uninstalled from KDE!"
echo ""
echo "If rtimer still appears in menus, try:"
echo "  • Press Alt+F2, type 'kquitapp5 plasmashell && plasmashell'"
echo "  • Or simply log out and log back in"
echo ""
echo "Thank you for using rtimer! 👋"