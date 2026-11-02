# Guia de implementação do Spotify no mothh

> Alternativa anterior. Para a direção atual, veja [música por pedidos e fila compartilhada](musica-implementacao.md).

Revisão: 2 de novembro de 2026. Base: `main`, commit `0e0ecfcdeaa5ba2253afcc6cbba07f8a90e7539d`, confirmado com `git fetch origin main`. O checkout já coincide com o remoto; alterações locais existentes foram preservadas. Este documento propõe a implementação; não adiciona integração funcional.

## 1. Resultado da análise do sistema

O mothh é um desktop Rust com Slint, sem webview. A UI roda na thread principal, a rede usa Tokio e WebSocket, e as chamadas usam LiveKit. Já existem módulos opcionais, compartilhamento do áudio da tela, MLS e cifra de mídia.

A integração musical ainda é planejamento: não há dependência `rspotify`/`librespot`, OAuth, comandos Spotify ou estado de sessão musical implementados. Os documentos `produto/musica.md` e `decisoes/0010-musica-via-spotify.md` propõem controlar o Spotify individual de cada participante com `rspotify`.

Pontos de extensão encontrados:

| Arquivo | Responsabilidade proposta |
| --- | --- |
| `crates/client/src/main.rs` | Registrar callbacks da UI e encaminhar comandos |
| `crates/client/src/net.rs` | Integrar eventos de sessão musical à rede existente |
| `crates/client/src/modules.rs` | Adicionar módulo opcional de música |
| `crates/client/ui/app.slint` | Entrada para componentes e estados da interface |
| `crates/protocol/src/lib.rs` | Tipar comandos e eventos compartilhados |
| `crates/server/src/ws.rs` | Despachar comandos autenticados |
| `crates/server/src/state.rs` | Manter sessões transitórias por conversa |
| `crates/client/src/share_audio.rs`, `system_audio.rs`, `call.rs` | Conferir interação com captura de áudio e chamadas |

## 2. Qual biblioteca usar

**Recomendação: adaptador local para a Web API, inicialmente com `rspotify`, com alternativa em `reqwest`.** O projeto já usa `reqwest` e Tokio. Isolar a biblioteca atrás de uma interface própria evita espalhar seus tipos pelo aplicativo. O README do `rspotify` informa modo de manutenção: escolher e fixar uma versão somente após conferir PKCE, refresh, modelos e endpoints em um teste real. [Repositório oficial do rspotify](https://github.com/ramsayleung/rspotify).

`rspotify` controla um player Spotify existente; não decodifica nem reproduz áudio. O som sai no dispositivo Spotify escolhido por cada pessoa, que pode ser o desktop ou outro dispositivo Connect disponível.

`librespot` é outro projeto: implementa um cliente/receptor Connect em Rust e requer Premium. Não é o SDK oficial do Spotify. Seu suporte técnico a reprodução não demonstra autorização da plataforma para distribuir essa experiência no mothh. Não adotá-lo como caminho padrão sem resolver essa questão. [Repositório oficial do librespot](https://github.com/librespot-org/librespot).

## 3. Condições de acesso e uso

O modo de desenvolvimento exige Premium do dono do aplicativo e permite até cinco usuários autenticados autorizados na allowlist; esse limite é do aplicativo, não de cada sala. Login concluído não garante acesso: uma conta fora da lista pode receber 403. Acesso ampliado exige análise; os critérios publicados incluem organização registrada e pelo menos 250 mil usuários ativos mensais. Não pressupor aprovação. [Quota modes](https://developer.spotify.com/documentation/web-api/concepts/quota-modes).

A documentação de fevereiro não é suficiente isoladamente: em julho de 2026 o limite de Client IDs passou a 25 por desenvolvedor e as quotas de desenvolvimento passaram a ser compartilhadas por conta do desenvolvedor. Isso não amplia a allowlist de um app nem justifica dividir o mothh entre Client IDs. [Mudanças de julho](https://developer.spotify.com/documentation/web-api/references/changes/july-2026).

Os controles de reprodução exigem Premium do usuário controlado. Cada participante deve autorizar sua própria conta e escolher seu próprio dispositivo. A API não oferece início simultâneo com horário agendado, e a ordem entre comandos Player não é garantida. [Start/Resume Playback](https://developer.spotify.com/documentation/web-api/reference/start-a-users-playback).

**Pendência de produto:** a política restringe broadcasting, integração com streams de outros serviços, sincronização com vídeo, sobreposição com outros áudios e reprodução de experiências centrais do Spotify. Minha interpretação é que o uso em um app com voz, câmera e tela exige validação específica; contas Premium separadas não comprovam, por si, conformidade. O ADR atual chama o fluxo de “forma permitida”, mas essa afirmação não foi demonstrada. Obter esclarecimento do Spotify para o fluxo concreto antes de liberar. [Developer Policy, seção III](https://developer.spotify.com/policy).

O Spotify Jam é uma alternativa oficial para ouvir em grupo. Uma primeira entrega pode permitir compartilhar manualmente um convite e abri-lo no Spotify. A documentação consultada não apresenta uma API pública de gerenciamento de Jam: não prometer criar Jam, listar seus membros ou controlá-lo pelo mothh. [Suporte oficial do Jam](https://support.spotify.com/us/article/jam/).

## 4. Funcionalidades que podem orientar a interface

Os endpoints abaixo são capacidades documentadas, não confirmação de disponibilidade para um Client ID específico. A etapa inicial deve verificar os acessos reais no Dashboard e nas contas de teste.

| Função | Web API / escopo | Consequência visual |
| --- | --- | --- |
| Conectar conta | OAuth Authorization Code com PKCE | Conectar, aguardando navegador, conectado, reconectar, desconectar |
| Ver reprodução e dispositivos | `GET /me/player`, `GET /me/player/devices`; `user-read-playback-state` | Faixa, artista, progresso, seletor de dispositivo |
| Reproduzir e pausar | `PUT /me/player/play`, `/pause`; `user-modify-playback-state` | Play/pause condicionado à disponibilidade e ao papel na sessão |
| Avançar/voltar | `POST /me/player/next`, `/previous`; mesmo escopo | Ações opcionais; “voltar” segue a semântica do Spotify |
| Mudar posição | `PUT /me/player/seek`; mesmo escopo | Barra interativa para quem conduz, progresso para os demais |
| Transferir reprodução | `PUT /me/player`; mesmo escopo | Escolher onde ouvir; transferência individual e explícita |
| Volume | `PUT /me/player/volume`; mesmo escopo | Volume individual, somente se suportado |
| Buscar e resolver música | `GET /search`, `GET /tracks/{id}` | Começar por colar link de faixa; busca em uma etapa posterior |
| Ler/adicionar à fila | `GET /me/player/queue`, `POST /me/player/queue`; escopos de leitura/escrita | Fila individual; fila de grupo exige coordenação própria |
| Shuffle/repeat | Endpoints Player correspondentes; mesmo escopo de escrita | Adiar para não conflitar com a sequência do grupo |

Referências dos controles: [índice da Web API](https://developer.spotify.com/documentation/web-api/reference), [transferência](https://developer.spotify.com/documentation/web-api/reference/transfer-a-users-playback).

Nem todo dispositivo aparece na API. `is_restricted` impede comandos; `supports_volume` determina se deve existir controle de volume. IDs de dispositivo podem mudar: redescobrir após falhas. Não exigir especificamente o app desktop quando outro Connect válido estiver disponível. [Dispositivos disponíveis](https://developer.spotify.com/documentation/web-api/reference/get-a-users-available-devices).

## 5. Autenticação e proteção das credenciais

Proposta de fluxo para o desktop:

1. Registrar Client ID e callback loopback no Dashboard.
2. Abrir navegador do sistema com Authorization Code + PKCE S256, `state` aleatório e escopos mínimos: `user-read-playback-state` e `user-modify-playback-state`.
3. Receber callback em listener temporário restrito a loopback. Validar `state`, caminho e autorização pendente; tratar cancelamento e timeout.
4. Trocar código usando `code_verifier`. Não embutir client secret no executável.
5. Guardar refresh token no cofre de credenciais do sistema, separado por perfil mothh/conta Spotify; access token em memória. Não reutilizar o arquivo JSON da sessão mothh para armazenar tokens em texto.
6. Renovar com exclusão mútua para evitar refresh concorrente. Nunca registrar tokens ou códigos em logs.
7. Desconectar cancela tarefas, apaga credenciais e dados associados, sai da sessão musical e informa como revogar o acesso na conta Spotify.

Adicionar escopos de perfil/biblioteca somente quando uma funcionalidade comprovadamente precisar deles. Não depender exclusivamente do campo de assinatura do perfil para habilitar playback; tratar também as respostas reais da API.

Fontes: [PKCE](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow), [redirect URIs](https://developer.spotify.com/documentation/web-api/concepts/redirect_uri). Usar IP literal, como `127.0.0.1`; `localhost` não é permitido. A política exige mecanismo acessível para desconectar e excluir dados relacionados. [Developer Policy, seção I](https://developer.spotify.com/policy).

## 6. Arquitetura proposta para ouvir junto

Esta seção é desenho de engenharia para um protótipo condicionado à validação de uso acima; não descreve uma capacidade nativa de grupos da Web API.

**Cliente:** criar `spotify.rs` com um adaptador assíncrono (`connect`, `devices`, `playback`, `play`, `pause`, `seek`, `disconnect`). Criar `music.rs` para regras da sessão. Todo HTTP roda no Tokio; a UI recebe estados pelo mecanismo existente de atualização no event loop do Slint.

**Servidor:** criar `music.rs` com sessão transitória por conversa: ID, geração, revisão crescente, condutor, participantes optantes e estado desejado. O servidor valida associação ao grupo e permissão em cada comando. Não recebe tokens Spotify nem áudio. Metadados de escuta são sensíveis: definir consentimento, retenção curta e se o conteúdo dos eventos seguirá a proteção MLS; não anunciar E2EE desses eventos sem implementá-la.

**Protocolo proposto:** `MusicCreate`, `MusicJoin`, `MusicLeave`, `MusicSetTrack`, `MusicSetPlaying`, `MusicSeek`, `MusicEnd`; respostas `MusicSnapshot`, `MusicParticipantStatus`, `MusicError`. Incluir conversa, ID de sessão, geração e revisão. Comandos de alteração carregam revisão esperada; rejeitar mudanças obsoletas e fora de ordem. Não confiar em IDs de condutor fornecidos pelo cliente.

**Estado compartilhado:** URI da faixa, posição em ms, tocando/pausado, instante de referência do servidor e duração quando conhecida. Dispositivo, volume e credenciais permanecem locais. Cada participante confirma entrada explicitamente; a presença no grupo não inicia playback automaticamente.

**Sincronização proposta:**

- Estimar diferença de relógio por ping/RTT e usar relógio monotônico local para avançar posição.
- Ao entrar, obter snapshot atual e iniciar a faixa na posição estimada. Limitar posição à duração.
- Usar `play` com URI e posição numa única solicitação quando possível; serializar comandos por conta e conferir o estado observado depois.
- Distinguir estado desejado do grupo e estado confirmado local. HTTP 204 sozinho não significa que alguém está ouvindo.
- Só corrigir desvio persistente. Hipótese inicial de teste: tolerância de 1,5 s e intervalo mínimo de 10 s entre correções; ajustar medindo a experiência, sem prometer precisão de estúdio.
- Progresso visual avança localmente. Consultas remotas são adaptativas, com pausa quando não há sessão; definir frequência após medir quotas reais, evitando polling agressivo por participante.
- Mudança manual no Spotify de um seguidor deve interromper o acompanhamento e oferecer “voltar a acompanhar”, evitando uma disputa de comandos.
- Condutor desconectado encerra ou suspende a sessão; transferência de condução exige ação explícita. Queda do WebSocket suspende controle automático, e reconexão pede snapshot novo.
- Saída para de acompanhar; não pausa o Spotify da pessoa automaticamente. Encerrar a sessão pode oferecer pausa individual explícita.

O áudio segue Spotify → dispositivo de cada pessoa. Não enviar o áudio Spotify ao LiveKit, nem apresentar captura de sistema como alternativa dessa integração. Para mídia própria, criar um módulo separado com requisitos de direitos definidos; “grupo privado” não basta para demonstrar autorização de retransmissão.

## 7. Estados e erros que o design precisa representar

| Estado | Texto/ação sugerida |
| --- | --- |
| Módulo desligado | Habilitar música nas configurações |
| Conta desconectada | Conectar Spotify |
| Autorização em andamento | Conclua no navegador; cancelar |
| Nenhum dispositivo | Abra o Spotify em um dispositivo e atualize |
| Dispositivo restrito | Escolher outro dispositivo |
| Premium necessário | Explicar requisito; abrir no Spotify |
| Acesso recusado | Verificar autorização/allowlist; não assumir que todo 403 é falta de Premium |
| Entrando | Aguardando confirmação da reprodução |
| Acompanhando | Sair; volume local se disponível |
| Divergência local | Voltar a acompanhar |
| Faixa indisponível | Informar indisponibilidade para esta conta/região |
| Sessão privada | Não compartilhar atividade automaticamente |
| Token expirado/revogado | Tentar uma renovação; reconectar se necessário |
| Rate limit | Esperar `Retry-After`; impedir tentativas repetidas |
| Quota excedida | Suspender consultas automáticas e informar indisponibilidade temporária |
| Sessão encerrada | Remover indicador de acompanhamento |

Distinguir `429` comum e `reason=QUOTA_EXCEEDED`; não entrar em loop de retries. Retentar operações de escrita com cuidado, principalmente pular faixa/adicionar à fila, que podem duplicar efeitos. [Rate limits](https://developer.spotify.com/documentation/web-api/concepts/rate-limits), [quotas de julho](https://developer.spotify.com/documentation/web-api/references/changes/july-2026).

## 8. Direção visual baseada nessas capacidades

Proposta: cartão compacto no topo da conversa com capa original, título, artista, atribuição Spotify, “abrir no Spotify”, estado e ação principal. Um painel expandido reúne dispositivo, participantes e ações disponíveis. O condutor tem controles do grupo; o seguidor tem entrada/saída e volume individual. Indicar “acompanhando” somente após confirmação local recente; o contador representa participantes confirmados no mothh, não estatística oficial Spotify.

**Alteração necessária na ideia do vinil:** manter o disco como ilustração própria, com rótulo original do mothh. A capa Spotify fica ao lado, inteira e estática. As diretrizes proíbem cortar, animar, distorcer ou sobrepor elementos à capa; portanto não aplicar recorte circular, rotação, dither ou filtro preto e branco à arte. Mostrar marca e links de atribuição conforme as regras. Não criar animação sincronizada por batida/análise do áudio. [Design & Branding Guidelines](https://developer.spotify.com/documentation/design).

Não reservar na primeira versão telas de letras, download offline, equalizador, recomendações algorítmicas, análise de áudio ou controle de Jam: não há suporte validado para essas entregas nesta pesquisa.

## 9. Sequência de implementação e critério de pronto

1. **Validar acesso e uso:** registrar app, verificar Dashboard/allowlist e testar endpoints com duas contas Premium próprias. Documentar respostas e esclarecer o cenário com chamadas. Alternativa imediata: compartilhar convite de Jam manualmente.
2. **Conexão individual:** PKCE, cofre, refresh, desconexão e seleção de dispositivo, com estados Slint mínimos.
3. **Controle individual:** faixa por link, leitura de estado, play/pause/seek e erros. Conferir compatibilidade do `rspotify`; usar adaptador `reqwest` se algum modelo/endpoint estiver desatualizado.
4. **Sessão de grupo experimental:** protocolo versionado, autorização no servidor, condutor único, entrada explícita e snapshots. Liberar para a allowlist somente após resolver a condição de uso.
5. **Sincronização:** medir entrada tardia, divergência, troca manual, perda de rede e quotas. Ajustar tolerâncias sem prometer reprodução perfeitamente simultânea.
6. **Interface final:** desenhar usando a matriz de capacidades e estados já demonstrados. Busca/fila colaborativa ficam para depois.

Validação necessária na implementação: testes de `state`/PKCE, isolamento de credenciais entre perfis, refresh concorrente, ordenação de revisões, autorização de membros e cálculo de posição. Testes manuais com duas contas e dispositivos reais: pausa, seek, mudança de faixa, entrada tardia, Premium ausente, fora da allowlist, dispositivo indisponível, 401/403/429, quota, reconexão e saída do condutor. Conferir ausência de tokens nos logs e ausência de publicação de áudio Spotify no LiveKit.

Esta entrega foi uma revisão de código e documentação pública. Não houve login Spotify, teste de endpoints com credenciais, execução do app ou validação da interface em funcionamento. O desenho de sincronização permanece uma proposta a comprovar.
