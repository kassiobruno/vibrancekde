#!/bin/sh
# Remove o vibrance-kde e devolve os monitores ao estado normal.
set -eu
BIN="$HOME/.local/bin/vibrance-kde"
if [ -x "$BIN" ]; then
  "$BIN" reset 2>/dev/null || true
  "$BIN" uninstall || true
fi
pkill -x vibrance-kde 2>/dev/null || true
rm -f "$BIN"
echo "Removido. A configuração em ~/.config/vibrance-kde foi mantida; apague-a se quiser."
