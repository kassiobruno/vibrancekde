#!/bin/sh
# Monta o AppDir e gera o AppImage a partir do binário de release.
#
# Pré-requisito: `cargo build --release` já executado.
# Variáveis: VERSION (padrão: versão do Cargo.toml), ARCH (padrão: x86_64).
#
# As bibliotecas gráficas (Wayland, xkbcommon, OpenGL) NÃO são empacotadas: o
# programa as carrega do sistema em tempo de execução, o que é o correto para
# GPU/Mesa e para integração com o KDE.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ARCH=${ARCH:-x86_64}
VERSION=${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)}
APPDIR="$ROOT/target/AppDir"
OUT="$ROOT/target/vibrance-kde-${VERSION}-${ARCH}.AppImage"
BIN="$ROOT/target/release/vibrance-kde"

[ -x "$BIN" ] || { echo "binário de release não encontrado: $BIN (rode cargo build --release)"; exit 1; }

rm -rf "$APPDIR"
install -Dm755 "$BIN" "$APPDIR/usr/bin/vibrance-kde"
install -Dm644 "$ROOT/packaging/vibrance-kde.desktop" "$APPDIR/usr/share/applications/vibrance-kde.desktop"
install -Dm644 "$ROOT/packaging/vibrance-kde.desktop" "$APPDIR/vibrance-kde.desktop"
install -Dm644 "$ROOT/assets/icons/vibrance-kde.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/vibrance-kde.svg"
install -Dm644 "$ROOT/assets/icons/vibrance-kde.svg" "$APPDIR/vibrance-kde.svg"
install -Dm644 "$ROOT/LICENSE" "$APPDIR/usr/share/licenses/vibrance-kde/LICENSE"

# miniatura PNG (.DirIcon) quando houver um conversor de SVG
if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w 256 -h 256 "$ROOT/assets/icons/vibrance-kde.svg" -o "$APPDIR/vibrance-kde.png"
  cp "$APPDIR/vibrance-kde.png" "$APPDIR/.DirIcon"
else
  cp "$APPDIR/vibrance-kde.svg" "$APPDIR/.DirIcon"
fi

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/vibrance-kde" "$@"
EOF
chmod 755 "$APPDIR/AppRun"

TOOL=${APPIMAGETOOL:-}
if [ -z "$TOOL" ]; then
  TOOL="$ROOT/target/appimagetool-$ARCH.AppImage"
  if [ ! -x "$TOOL" ]; then
    curl -fsSL -o "$TOOL" \
      "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
    chmod +x "$TOOL"
  fi
fi

# --appimage-extract-and-run evita depender de FUSE (CI, contêineres)
ARCH="$ARCH" "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT"
chmod +x "$OUT"
echo "gerado: $OUT"
