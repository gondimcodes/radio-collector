# Radio Collector (Rust + TUI)

*Leia isto em [English](README.md).*

Coletor autônomo e inteligente de faixas de rádio online desenvolvido em **Rust**. 

![Radio Collector TUI](docs/images/tui-screenshot.png)

Em vez de ripar fluxos contínuos de áudio de baixa qualidade com vinhetas coladas, o **Radio Collector** monitora apenas os metadados ICY da rádio em tempo real e baixa a versão oficial de estúdio em **MP3 320 kbps (48 kHz)** diretamente de fontes de catálogo oficial (YouTube Music), embutindo capa do álbum em alta resolução e tags ID3 completas.

---

## Recursos Principais

* **Interface Visual TUI (`ratatui`)**: Dashboard interativo no terminal exibindo a música tocando no momento, transições discretas de estado (`BAIXANDO`/`BAIXADO`), histórico recente, atalhos para faixas pendentes (`[Enter/b]` para baixar, `[r/d]` para recusar/excluir) e contadores sincronizados com o disco.
* **Operação em Modo Duplo Inteligente**:
  * **Modo TUI Interativo**: Executado diretamente no terminal para monitoramento visual. Detecta automaticamente se o daemon do `systemd` já está ativo e opera como **Monitor** sem disparar downloads duplicados.
  * **Modo Daemon Headless (`--daemon`)**: Executado via `systemd` em background 24/7 sem renderizar interface gráfica, gerando logs estruturados no `journald`.
* **Zero Desperdício de Banda**: Conecta ao servidor Icecast/SHOUTcast, lê apenas os primeiros bytes de metadados da faixa e fecha o socket imediatamente (~1 KB de tráfego por consulta).
* **Deduplicação Inteligente em 2 Camadas**:
  * **Banco SQLite Embutido**: Histórico persistente com normalização de strings (*case-insensitive* e remoção de espaços extras).
  * **Inspeção e Reconciliação Física no Disco**: Varredura direta no diretório de destino na inicialização, corrigindo inconsistências e garantindo que o acervo físico e o banco estejam sempre alinhados.
* **Filtro de Vinhetas e Comerciais**: Expressões regulares configuráveis para descartar chamadas da emissora, propagandas ou faixas inválidas.
* **Resiliência e Proteção contra Falhas de Extração**:
  * Conversão automática de capas WebP para JPEG antes da injeção de ID3 (evitando quebras do `ffprobe`).
  * Auto-expurgo de arquivos temporários e sobras parciais (`.webm`, `.webp`, `.part`) em caso de interrupções.

---

## Estrutura do Projeto

```
radio-collector/
├── Cargo.toml               # Dependências e manifesto da aplicação Rust
├── config.toml              # Arquivo de configuração de exemplo
├── radio-collector.service  # Unidade systemd para execução em background
├── INSTALL.md               # Guia de instalação em inglês
├── INSTALL.pt-BR.md         # Guia de compilação e instalação no Debian 13 (pt-BR)
├── README.md                # Manual de uso e arquitetura em inglês
├── README.pt-BR.md          # Manual de uso e arquitetura (pt-BR)
└── src/
    ├── main.rs              # Ponto de entrada, detecção de worker e transição de modos
    ├── config.rs            # Parser fortemente tipado do arquivo config.toml
    ├── db.rs                # Camada SQLite com WAL mode, reconciliação e deduplicação
    ├── downloader.rs        # Executor assíncrono do yt-dlp com stream de progresso e conversão JPEG
    ├── events.rs            # Tipos de eventos trocados entre workers e interface
    ├── filter.rs            # Validador e filtros regex de vinhetas
    ├── icy.rs               # Cliente assíncrono TCP para extração de metadados ICY
    └── tui.rs               # Interface gráfica de terminal baseada em Ratatui
```

---

## Como Usar

### 1. Execução no Modo TUI (Interativo)
Para abrir o painel visual no seu terminal ou sessão SSH:

```bash
# Utiliza automaticamente /etc/radio-collector/config.toml (ou ./config.toml)
radio-collector

# Ou especificando um arquivo de configuração customizado:
radio-collector --config /caminho/meu_config.toml
```

#### Teclas de Atalho na TUI:
* **`↑` / `↓`** (ou **`k` / `j`**): Navega e seleciona faixas na tabela de histórico com cursor indicador (`▶`).
* **`d`** ou **`Delete`**: Abre a janela modal flutuante de confirmação para excluir a música selecionada (`[S]` confirma e remove do banco/disco, `[N]` ou `Esc` cancela).
* **`q`** ou **`Esc`**: Encerra a aplicação de forma graciosa e restaura o terminal.

---

### 2. Execução no Modo Daemon (Background / Servidor)
Para rodar silenciosamente em segundo plano (como serviço systemd):

```bash
radio-collector --daemon
```

Nesse modo, a interface gráfica é desativada e todas as operações são registradas no formato de log padrão:
```text
[*] Radio Collector iniciado em modo DAEMON (Config: "/etc/radio-collector/config.toml")
[*] Monitorando: http://sokakarecos.ddns.net:9005/light128
[*] Destino de Áudio: /var/musicas
[NOW PLAYING] Rádio transmitindo: 'Bee Gees - Love So Right'
[DOWNLOAD] Iniciando busca e download em 320k: 'Bee Gees - Love So Right'
[SUCESSO] Faixa gravada com capa e ID3: 'Bee Gees - Love So Right'
```

---

## Configuração (`/etc/radio-collector/config.toml`)

O comportamento do coletor é totalmente controlado via arquivo TOML:

```toml
[stream]
url = "http://sokakarecos.ddns.net:9005/light128" # URL da rádio
user_agent = "VLC/3.0.18 LibVLC/3.0.18"          # User-Agent para a rádio
check_interval_seconds = 30                       # Intervalo de consulta
timeout_seconds = 10                              # Timeout da conexão ICY

[storage]
output_dir = "/var/musicas"                       # Onde salvar os MP3s
db_path = "/var/lib/radio-collector/musicas.db"   # Banco SQLite

[download]
ytdlp_path = "/usr/local/bin/yt-dlp"              # Caminho do binário yt-dlp
audio_format = "mp3"                              # Formato final
audio_quality = "0"                               # VBR 0 (~320 kbps)
timeout_seconds = 180                             # Tempo limite por música
embed_metadata = true                             # Injeta tags ID3 (artista, título)
embed_thumbnail = true                            # Injeta capa oficial do álbum
auto_download = true                              # Baixa direto (true) ou coloca como PENDENTE (false)

[filter]
min_title_length = 4                              # Tamanho mínimo do título
require_hyphen = true                             # Exige formato "Artista - Faixa"
ignore_patterns = [                               # Regex para ignorar vinhetas
    "kakarecos",
    "so\\s*kakarecos",
    "vinheta",
    "comercial",
    "chamada",
    "radio\\s*light",
]

[ui]
tui = true                                        # Habilita TUI por padrão no terminal
```

---

## Requisitos de Execução

* **Linux**: Debian 13 (Trixie) ou qualquer distribuição moderna x86_64 / aarch64.
* **`yt-dlp`**: Instalado em `/usr/local/bin/yt-dlp` (ou configurado no `config.toml`).
* **`ffmpeg`**: Necessário no sistema para que o `yt-dlp` converta a faixa para MP3 e faça o encode de capas.
* **`nodejs`**: Runtime JavaScript necessário para que o `yt-dlp` decifre desafios do YouTube (EJS / n-sig).

