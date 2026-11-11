#!/bin/sh
# Compila e instala o vibrance-kde para o usuário atual (sem sudo).
set -eu
cd "$(dirname "$0")"

command -v cargo >/dev/null 2>&1 || { echo "Rust/cargo não encontrado. Instale em https://rustup.rs"; exit 1; }
command -v kscreen-doctor >/dev/null 2>&1 || echo "Aviso: kscreen-doctor não encontrado (pacote libkscreen / kscreen)."

cargo build --release
install -Dm755 target/release/vibrance-kde "$HOME/.local/bin/vibrance-kde"
"$HOME/.local/bin/vibrance-kde" install

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "Aviso: $HOME/.local/bin não está no PATH desta sessão." ;;
esac
echo
echo "Instalado. Abra 'vibrance-kde' pelo menu de aplicativos ou execute: vibrance-kde"
echo "Para iniciar com o sistema: marque a opção na janela, ou rode: vibrance-kde autostart on"
