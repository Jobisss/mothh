#!/bin/sh
# Acompanha o log do servidor do mothh ao vivo, na VPS, na pasta do projeto.
#
#   ./infra/logs.sh             erros e avisos (o que precisa de atenção)
#   ./infra/logs.sh chamada     chamadas e troca de chaves: quem entrou, com que
#                               época da chave, rota, perda, falhas de cifra
#   ./infra/logs.sh <usuário>   tudo de uma pessoa
#   ./infra/logs.sh tudo        tudo
#   ./infra/logs.sh livekit     o LiveKit (mídia): conexões, perdas, quedas
#
# Para ver o passado em vez de ao vivo: DESDE=3h ./infra/logs.sh chamada
set -eu
cd "$(dirname "$0")/.."
desde="${DESDE:-30m}"
compose="docker compose -f docker-compose.prod.yml"
modo="${1:-erros}"
case "$modo" in
  erros)   $compose logs -f --no-log-prefix --since "$desde" server | grep --line-buffered -E " (WARN|ERROR) " ;;
  chamada) $compose logs -f --no-log-prefix --since "$desde" server | grep --line-buffered -E " (app|server::ws): (chamada|mls|som|rede|erro)| WARN | ERROR " ;;
  tudo)    $compose logs -f --no-log-prefix --since "$desde" server ;;
  livekit) $compose logs -f --no-log-prefix --since "$desde" livekit | grep --line-buffered -viE "debug" ;;
  *)       $compose logs -f --no-log-prefix --since "$desde" server | grep --line-buffered -F "username=$modo" ;;
esac
