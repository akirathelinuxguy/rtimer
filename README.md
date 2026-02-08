# rtimer

**rtimer** is a lightweight timer application written in **Rust**, designed for Linux desktops.  
It includes simple install and uninstall scripts and desktop integration (KDE-friendly).

The project focuses on being minimal, fast, and easy to install.

---

## ✨ Features

- Simple timer utility
- Written in Rust
- Desktop entry and icon included
- Install and uninstall scripts
- Lightweight and dependency-minimal

---

##  Installation

Clone the repository:


git clone https://github.com/Reuben-Percival/rtimer.git

cd rtimer

Build the project:

cargo build --release
Install system-wide (may require root):

sudo ./install.sh

## Uninstallation

To remove rtimer from your system:

sudo ./uninstall.sh

## Usage
After installation, you can:

Launch rtimer from your application menu

Or run it directly from the terminal (if installed in PATH):
rtimer

### Development

Requirements:

Rust 

Cargo

To run during development:

cargo run

# License

This project is licensed under the GNU General Public License v3.0 As it protects my project
See the LICENSE file for details.

