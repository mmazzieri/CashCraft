#!/bin/sh
# Doppio clic: crea CashCraft per questo Mac e la mette nelle Applicazioni (Launchpad).
# Double-click: builds CashCraft for this Mac and installs it into Applications.
cd "$(dirname "$0")"
exec sh packaging/macos/package.sh --arch native --install
