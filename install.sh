#!/bin/bash

set -e

echo "🍅 Installing rtimer - Beautiful Pomodoro Timer for KDE"
echo ""

# Build the release version
echo "📦 Building rtimer..."
cargo build --release

# Install binary
echo "📥 Installing binary..."
sudo cp target/release/rtimer /usr/local/bin/

# Install KDE desktop entry
echo "🖥️  Installing KDE desktop entry..."
mkdir -p ~/.local/share/applications
cp rtimer-kde.desktop ~/.local/share/applications/rtimer.desktop

# Install icon
echo "🎨 Installing icon..."
mkdir -p ~/.local/share/icons/hicolor/scalable/apps
cp rtimer.svg ~/.local/share/icons/hicolor/scalable/apps/rtimer.svg

# Update KDE caches
echo "🔄 Updating KDE caches..."
if command -v kbuildsycoca5 &> /dev/null; then
    kbuildsycoca5 2>/dev/null || true
fi

if command -v kbuildsycoca6 &> /dev/null; then
    kbuildsycoca6 2>/dev/null || true
fi

# Alternative update methods
if command -v update-desktop-database &> /dev/null; then
    update-desktop-database ~/.local/share/applications
fi

echo ""
echo "✅ Installation complete!"
echo ""
echo "KDE Features:"
echo "  • Launches in Konsole terminal"
echo "  • Proper KDE menu integration"
echo "  • System tray ready (for future updates)"
echo ""
echo "You can now:"
echo "  • Run 'rtimer' from terminal"
echo "  • Launch from KDE Application Menu"
echo "  • Add to KDE Panel or Desktop"
echo "  • Use Alt+F2 and type 'rtimer'"
echo ""
echo "Usage examples:"
echo "  rtimer                    # Start with default settings"
echo "  rtimer -w 50 -r 10       # Custom durations"
echo "  rtimer --help            # See all options"
echo ""