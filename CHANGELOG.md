# Changelog

## 0.1.2

- **Correção:** depois de abrir o programa pela primeira vez na sessão ou de reiniciar o PC, o
  jogo abria com a cor padrão (a barra de vibrance só "pegava" depois de ser movida). O KWin
  guarda o caminho do último perfil ICC, mas perde o perfil em memória no reinício (o arquivo já
  tinha sido apagado). Como o `kscreen-doctor` só reenvia o perfil quando o caminho muda, o
  mesmo nível gerava o mesmo caminho e o KWin nunca o carregava. Agora cada aplicação gera um
  arquivo com nome inédito (`vibrance-kde-<monitor>-<nível>-n<sufixo>.icc`).

## 0.1.1

- Interface: os blocos "Vibrance", "Programas" e "Monitores" passam a ocupar a largura
  inteira da janela (antes tinham larguras diferentes e ficavam desalinhados).
- README com capturas de tela e os ícones da bandeja.
- Release: `SHA256SUMS` não lista mais a si mesmo e o pacote `-debug` não é mais anexado.
- Novo modo de desenvolvimento para gerar as capturas (`--features screenshots`), fora dos
  binários e pacotes publicados.

## 0.1.0

- Primeira versão: daemon com bandeja, script de foco no KWin, perfil ICC em Rust puro,
  janela de configurações, autostart minimizado, troca de resolução por jogo, AppImage e
  PKGBUILD para o AUR.
