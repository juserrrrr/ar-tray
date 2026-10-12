# Painel do Ar

Painel na bandeja do Windows para controlar um ar-condicionado Samsung pelo SmartThings, feito em Rust e desenhado direto na tela, sem navegador embutido.

O ícone na bandeja mostra a temperatura do ambiente, com uma barrinha na cor do modo quando o ar está ligado. Passando o mouse, aparecem também a temperatura programada e a umidade. Um clique abre o painel, e clicar fora ou apertar Esc fecha. O botão direito abre o menu com ligar e desligar, atualizar, abrir com o Windows e sair.

| Tema escuro | Tema claro | Conexão | Ajustes |
| --- | --- | --- | --- |
| ![Painel no tema escuro](docs/painel-escuro.png) | ![Painel no tema claro com WindFree](docs/painel-claro.png) | ![Tela para conectar a conta Samsung](docs/conectar.png) | ![Ajustes do aparelho](docs/ajustes.png) |

## Por que é leve

- **Sem navegador.** O painel é desenhado em Rust com [tiny-skia](https://github.com/linebender/tiny-skia) numa janela comum do Windows. Os cantos arredondados e a sombra são do próprio Windows 11.
- **Só redesenha o que muda.** A parte fixa do painel é desenhada uma vez por mudança. A cada quadro (30 por segundo, só com o painel aberto) entra apenas o vento animado.
- **Memória devolvida ao fechar.** Os quadros do painel são liberados quando ele fecha.
- **Sem Node e sem Electron.** O login e a API da SmartThings são chamados direto do Rust, e o TLS é o do próprio Windows.

Num PC com Windows 11, conectado a um ar, o exe tem cerca de 1,9 MB e roda num processo só, com uns 6 MB de memória privada com o painel fechado e uns 9 MB com ele aberto.

## Requisitos

- Windows 10 ou 11. No Windows 10 os cantos ficam retos.
- Ar-condicionado Samsung cadastrado no app SmartThings.
- Para compilar: Rust (toolchain MSVC) e o Visual Studio Build Tools com C++.

## Compilar e rodar

```
cargo build --release
target\release\ar-tray.exe
```

Na primeira vez, o painel abre na tela **Conecte sua conta Samsung**. O botão **Entrar com a conta Samsung** abre o login no navegador, e o painel fica aberto ao lado mostrando o andamento, com opções para abrir a página de novo ou cancelar. Com um ar-condicionado na conta, ele já aparece; com vários, o painel pergunta qual controlar. Abrir o exe de novo com ele já rodando só abre o painel.

O login usa o mesmo cliente da CLI oficial da SmartThings: ele se renova sozinho e só pede para entrar de novo depois de 30 dias sem uso.

Para ver o painel sem conta, com um aparelho de mentira:

```
target\release\ar-tray.exe --demo
```

## Testes

- `cargo test`: comandos, fila, telas, cliques e o ícone da bandeja.
- `cargo test smartthings -- --ignored`: chamadas reais à SmartThings, sem login.
- `cargo test preview -- --ignored`: salva imagens do painel em vários estados, nos dois temas, em `target\preview`.

## Usar o painel

- **Temperatura**: botões − e +, o controle deslizante, a roda do mouse sobre o painel ou as setas do teclado.
- **Modo**: um toque na fileira de modos. Aparecem só os modos que o aparelho aceita.
- **Vento e Direção**: abrem uma lista com as opções do aparelho.
- **WindFree, Sono e Turbo**: ligam e desligam direto. O Turbo é o resfriamento rápido da Samsung: roda por um tempo e volta sozinho para o ajuste anterior, sem mudar temperatura nem vento. Enquanto ele roda, o aparelho trava a temperatura e o vento, e o painel deixa esses controles apagados. Os três ficam apagados nos aparelhos que não têm.
- **Ajustes** (canto de baixo): luz do display, bipe, secagem automática, filtro, consumo, abrir com o Windows, trocar de aparelho e sair da conta Samsung.

O consumo não é ao vivo: o ar manda a potência à SmartThings a cada 5 minutos, então os ajustes mostram há quanto tempo ela foi medida.

O painel segue o tema claro ou escuro do Windows. Quando o Windows inicia o app, ele fica só no ícone, sem abrir o painel.

Mudanças feitas com o ar desligado ficam guardadas e são aplicadas quando ele liga. Cliques seguidos viram um comando só.

## Onde ficam os dados

Tudo fica em `%APPDATA%\dev.ar-tray.desktop`:

- `sessao.bin`: o login da SmartThings, criptografado com a conta do Windows (DPAPI).
- `device.json`: o aparelho escolhido. Apague para o app procurar de novo, ou edite para escolher outro:

  ```json
  { "id": "ID-DO-APARELHO", "nome": "Ar da sala" }
  ```

- `settings.json`: se o app abre com o Windows.
- `log.txt`: os passos do login e qualquer falha, com a linha do código quando o `.pdb` está ao lado do exe.

## Como funciona

- `src/smartthings.rs`: login no navegador e chamadas da API. Usa o mesmo cliente público (PKCE) da [CLI oficial da SmartThings](https://github.com/SmartThingsCommunity/smartthings-cli), que renova o acesso sozinho e só pede login de novo depois de 30 dias sem uso.
- `src/ac.rs`: transforma os ajustes em comandos e confere se o ar aplicou cada um.
- `src/bridge.rs`: estado da conta, fila de comandos e atualização, a cada 15 segundos com o painel aberto e 60 com ele fechado.
- `src/shell.rs`: ícone e menu da bandeja, janela do painel, pintura e entrada.
- `src/panel.rs`: layout, desenho e cliques do painel e das telas de conexão. `src/scene.rs` desenha o aparelho e o vento.
- `src/gfx.rs`: formas, degradês, texto com a Bahnschrift do Windows e ícones.
- `src/tray_icon.rs`: o número da bandeja.
- `tools/gerar-icones.mjs` gera `src/icons.rs` a partir do pacote `lucide`.

## Problemas comuns

- **"Sem resposta do SmartThings"**: confira no app SmartThings se o ar está online e clique em "Tentar de novo".
- **A tela de conexão voltou**: o login expirou depois de 30 dias sem uso. Entre de novo pelo botão.
- **"Nenhum ar-condicionado"**: o login precisa ser na mesma conta em que o ar aparece no app SmartThings.
- **"O ar não confirmou…"**: alguns modos não aceitam todos os ajustes. O painel volta para o estado real do aparelho.
- **Algo estranho aconteceu**: veja o `log.txt` na pasta de dados.

## Desinstalar

Desmarque "Abrir com o Windows" no menu do ícone, clique em **Sair** e apague o exe e a pasta `%APPDATA%\dev.ar-tray.desktop`.

## Créditos

Ícones do [Lucide](https://lucide.dev) (ISC). Samsung, WindFree e SmartThings são marcas da Samsung; este projeto não tem ligação com a empresa.
