#!/bin/bash

# rtimer Update Script
# This script pulls the latest changes from GitHub and rebuilds the application

set -e

echo "🔄 rtimer Update Script"
echo "========================"

# Check if we're in a git repository
if [ ! -d ".git" ]; then
    echo "❌ Error: Not in a git repository"
    echo "Please run this script from the rtimer directory"
    exit 1
fi

# Check for uncommitted changes
if ! git diff-index --quiet HEAD -- 2>/dev/null; then
    echo "⚠️  Warning: You have uncommitted changes"
    echo ""
    git status --short
    echo ""
    read -p "Continue with update? This may overwrite local changes (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Update cancelled"
        exit 0
    fi
fi

# Fetch latest changes
echo "📥 Fetching latest changes from GitHub..."
git fetch origin

# Check if there are updates
LOCAL=$(git rev-parse @)
REMOTE=$(git rev-parse @{u} 2>/dev/null || echo "")
BASE=$(git merge-base @ @{u} 2>/dev/null || echo "")

if [ -z "$REMOTE" ]; then
    echo "⚠️  Warning: No remote tracking branch found"
    read -p "Pull from origin/main? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        git pull origin main
    else
        echo "Update cancelled"
        exit 0
    fi
elif [ "$LOCAL" = "$REMOTE" ]; then
    echo "✓ Already up to date!"
    read -p "Rebuild anyway? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 0
    fi
elif [ "$LOCAL" = "$BASE" ]; then
    echo "📦 Updates available, pulling changes..."
    git pull
elif [ "$REMOTE" = "$BASE" ]; then
    echo "⚠️  Warning: Your local version is ahead of remote"
    read -p "Reset to remote version? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        git reset --hard @{u}
    else
        echo "Update cancelled"
        exit 0
    fi
else
    echo "⚠️  Warning: Branches have diverged"
    read -p "Reset to remote version? (y/N): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        git reset --hard @{u}
    else
        echo "Update cancelled"
        exit 0
    fi
fi

# Show what changed
echo ""
echo "📋 Recent changes:"
git log --oneline -5
echo ""

# Rebuild the application
echo "🔨 Rebuilding rtimer..."
cargo build --release

if [ $? -eq 0 ]; then
    echo ""
    echo "✅ Update complete!"
    echo ""
    echo "Changes have been applied and rtimer has been rebuilt."
    
    # Check if already installed
    if [ -f "$HOME/.local/bin/rtimer" ] || [ -f "/usr/local/bin/rtimer" ]; then
        echo ""
        read -p "Reinstall to system? (y/N): " -n 1 -r
        echo
        if [[ $REPLY =~ ^[Yy]$ ]]; then
            ./install.sh
        else
            echo "You can run ./install.sh later to update the system installation"
        fi
    fi
    
    echo ""
    echo "Run './target/release/rtimer' to use the updated version"
else
    echo ""
    echo "❌ Build failed. Please check the error messages above."
    exit 1
fi
