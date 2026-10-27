# Instalação: Radio Collector (Debian 13)

*Leia isto em [English](INSTALL.md).*

Este guia orienta a compilação, instalação e configuração do **Radio Collector** (Rust + TUI) no **Debian 13 (Trixie)** como binário nativo e serviço de background do sistema.

---

## 1. Dependências do Sistema Operacional

Instale as ferramentas de compilação do Rust, o utilitário de áudio `ffmpeg`, o interpretador JavaScript `nodejs` (exigido pelo `yt-dlp` moderno para decifração de assinaturas do YouTube) e dependências básicas do sistema:

```bash
apt update && apt upgrade -y
apt install -y build-essential pkg-config libssl-dev libsqlite3-dev \
               ffmpeg nodejs curl wget git
```

> [!IMPORTANT]
> **JavaScript Runtime (`nodejs`)**: O YouTube implementou desafios externos de JavaScript (EJS / n-sig) nas streams de áudio oficiais. Sem um runtime como `nodejs` instalado, o `yt-dlp` perde formatos de alta fidelidade e falha no pós-processamento de codecs de áudio com `ffprobe`. O pacote `nodejs` resolve essa exigência nativamente no Debian 13.

### Instalar o Toolchain Oficial do Rust (se ainda não instalado):
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustc --version
```

### Instalar o `yt-dlp` Oficial (Motor de Download):
```bash
wget https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -O /usr/local/bin/yt-dlp
chmod a+rx /usr/local/bin/yt-dlp
yt-dlp --version
```

---

## 2. Compilação do Binário em Rust

Dentro do diretório do projeto `radio-collector`:

```bash
cd /caminho/para/radio-collector

# Compilar com otimizações máximas para produção
cargo build --release
```

O binário otimizado e autocontido será gerado em:
`target/release/radio-collector`

Instale o binário no caminho padrão do sistema:
```bash
install -m 0755 target/release/radio-collector /usr/local/bin/radio-collector
```

---

## 3. Estrutura de Diretórios e Configuração

Crie os diretórios de configuração, dados e destino das músicas:

```bash
mkdir -p /etc/radio-collector
mkdir -p /var/lib/radio-collector
mkdir -p /var/musicas
```

Copie o arquivo de configuração para `/etc/radio-collector/`:

```bash
cp config.toml /etc/radio-collector/config.toml
chmod 0644 /etc/radio-collector/config.toml
```

### Ajuste os parâmetros conforme necessário em `/etc/radio-collector/config.toml`:
* `stream.url`: URL da rádio que você quer monitorar.
* `storage.output_dir`: Pasta onde os MP3s finais serão salvos (padrão `/var/musicas`).
* `filter.ignore_patterns`: Lista de palavras ou termos para descartar vinhetas.

---

## 4. Configurar como Serviço do Sistema (Systemd)

Para que o coletor rode silenciosamente 24 horas por dia em background:

1. Copie o arquivo de serviço:
```bash
cp radio-collector.service /etc/systemd/system/radio-collector.service
chmod 0644 /etc/systemd/system/radio-collector.service
```

> [!NOTE]
> **Permissão em `/home/` (`ProtectHome=false`)**: Se o seu `output_dir` estiver dentro do diretório `/home/` (ex: `/home/gondim/Músicas/kakarecos`), garanta que a diretiva `ProtectHome=false` esteja presente em `/etc/systemd/system/radio-collector.service`, caso contrário o systemd impedirá o `yt-dlp` de gravar os arquivos MP3.

2. Ative e inicie o serviço:
```bash
systemctl daemon-reload
systemctl enable --now radio-collector.service
```

3. Para acompanhar os logs de execução:
```bash
journalctl -u radio-collector.service -f
```

---

## 5. Interface Gráfica Interativa (TUI)

Você pode abrir a interface visual a qualquer momento, mesmo com o serviço do `systemd` ativo:

```bash
radio-collector
```

* **Com o serviço do systemd rodando**: A TUI entra automaticamente em **Modo Monitor**, observando os downloads em tempo real sem conflitar com o daemon.
* **Com o serviço parado**: A TUI assume a coleta standalone e baixa diretamente.
* **Navegação**: Teclas `↑`/`↓` ou `k`/`j` (delimitada, sem loop circular).
* **Paginação**: Teclas `PageUp`/`PageDown` (salta 15 faixas), `Home`/`End` (vai ao topo ou fim).
* **Tentar Novamente Faixas com Falha/Pendentes**: Tecle `t` ou `T` sobre qualquer faixa com `FALHA` ou `PENDENTE` para disparar novo download em background.
* **Baixar Faixa Pendente**: Tecle `Enter` ou `b`.
* **Exclusão Segura**: Selecione a faixa e tecle `d` ou `Delete` (abre modal de confirmação para remover da base e do disco).
* **Sair**: Tecle `q`.

---

## 6. Manutenção Automática dos Extratores (`yt-dlp`)

O YouTube altera periodicamente seus algoritmos de entrega de stream. Mantenha os extratores do `yt-dlp` atualizados automaticamente via `crontab` semanal:

```bash
(crontab -l 2>/dev/null; echo "0 4 * * 0 /usr/local/bin/yt-dlp -U >/dev/null 2>&1") | crontab -
```

