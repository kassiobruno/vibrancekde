# Avisos de terceiros

* A técnica de gerar um perfil ICC com tabela B2A (lut16) para aplicar saturação em espaço gama
  no KWin, e as constantes numéricas de `src/icc.rs` (matriz Bradford, escala de XYZ do KWin,
  offset e intervalo da CLUT), vêm do projeto
  [plasma-vibrance](https://github.com/Celiz/plasma-vibrance), licença MIT:

  > MIT License. Copyright (c) Celiz.
  > Permission is hereby granted, free of charge, to any person obtaining a copy of this software
  > and associated documentation files, to deal in the Software without restriction, including
  > without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense,
  > and/or sell copies of the Software, subject to the above copyright notice and this permission
  > notice being included in all copies or substantial portions of the Software. THE SOFTWARE IS
  > PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND.

  O código aqui é uma reescrita independente em Rust (sem liblcms2). A licença MIT é compatível
  com a GPL-3.0 deste projeto; o aviso acima é mantido por exigência da MIT.

* O comportamento (troca de foco, nível por programa, nível do desktop, restauração ao sair,
  autostart minimizado) foi reimplementado a partir da observação do funcionamento do
  vibranceGUI (juvlarN). Nenhum código dele foi copiado.
