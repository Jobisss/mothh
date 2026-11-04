# Back-end do inventário: proposta

Data: 04/11/2026. Status: proposta para discutir antes da fase 4. Produto em [produto/inventario.md](produto/inventario.md).

O pedido é um back-end "de finanças": nunca perder um item, nunca dar um item duas vezes, nunca deixar alguém se dar um item, e sempre conseguir explicar por que uma pessoa tem o que tem. Sem dinheiro envolvido, mas com a mesma disciplina.

## Princípios

1. **O servidor é a única fonte da verdade.** O app só lê o inventário e escolhe o que mostrar no perfil. Nenhuma mensagem do cliente cria item.
2. **Livro-razão, não tabela de estado.** Cada item ganho é um lançamento imutável (`inventory_ledger`). O inventário de uma pessoa é a soma dos lançamentos dela; nunca se edita nem se apaga uma linha. Correções são lançamentos novos (`revoke`), com motivo.
3. **Idempotência por chave natural.** Todo lançamento tem uma chave única `(user_id, item_id, occasion)`: "ana ganhou `natal-2026` na ocasião `2026-12-25`". Rodar a mesma regra duas vezes, reiniciar o servidor no meio, ou dois nós fazendo a mesma coisa, não duplica nada: o banco recusa pela unicidade e o código trata como "já tinha".
4. **Tudo numa transação.** Regra que concede item a várias pessoas de uma vez (um marco do grupo) grava todos os lançamentos na mesma transação, ou nenhum.
5. **Catálogo versionado no repositório.** Os itens existem primeiro como arquivos (`cosmeticos/<id>/item.toml` + o glTF), com um conferidor no CI. O servidor só conhece ids e as regras; o modelo 3D vai dentro do app. Item desconhecido pelo app aparece como "item novo: atualize o app", nunca some.
6. **Auditável.** Cada lançamento guarda quem ou o que concedeu (`source`: `rule:<nome>`, `admin:<user_id>`), quando, e um `note` curto. Dá para responder "por que a Ana tem isso" só olhando a tabela.

## Esquema

```sql
CREATE TABLE inventory_ledger (
    id BIGSERIAL PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    item_id TEXT NOT NULL,               -- id do catálogo, ex.: 'natal-2026'
    occasion TEXT NOT NULL,              -- o que torna este ganho único, ex.: '2026-12-25' ou 'grupo:<uuid>:100-mensagens'
    kind TEXT NOT NULL CHECK (kind IN ('grant', 'revoke')),
    source TEXT NOT NULL,                -- 'rule:natal' | 'admin:<uuid>'
    note TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Um ganho por pessoa, item e ocasião. O revoke referencia o grant pelo mesmo trio.
CREATE UNIQUE INDEX inventory_ledger_once ON inventory_ledger (user_id, item_id, occasion, kind);
CREATE INDEX inventory_ledger_user ON inventory_ledger (user_id, id);
```

Inventário materializado (opcional, só para leitura rápida): `inventory_items (user_id, item_id, occasion, granted_at)`, mantido na mesma transação do lançamento. Um job de reconciliação diário recalcula a partir do ledger e avisa no log se divergir; o ledger ganha.

A escolha do que aparece no perfil já tem lugar: `profiles.showcases` (texto do perfil). Entra uma vitrine `items` com até N ids de `(item_id, occasion)`; o servidor valida que a pessoa tem cada um.

## Como se ganha

| Regra | Gatilho | Ocasião | Observações |
| --- | --- | --- | --- |
| Data comemorativa | Job do servidor, uma vez por dia (fuso de Brasília) | A data (`2026-12-25`) | Só para quem entrou no app naquele dia ou na semana (evita contas mortas) |
| Marco do grupo | Evento no servidor (100ª mensagem, 1ª chamada com 5 pessoas) | `grupo:<id>:<marco>` | Todos os membros no momento, numa transação |
| Presente | Comando de administrador (o dono do servidor) | `presente:<n>` | `source = admin:<uuid>`, `note` obrigatório |

Regras são código no servidor (`crates/server/src/inventory/rules.rs`), não configuração solta: cada uma tem teste. Nada de endpoint "dar item" aberto ao app.

## Protocolo

- `GetInventory` → `Inventory { items: [{ item_id, occasion, granted_at }] }` (o seu; o de outra pessoa vem dentro de `Profile`, só o que ela pôs na vitrine).
- `SaveProfile` ganha a vitrine `items` dentro de `showcases`.
- Evento `ItemGranted { item_id, occasion }` para a pessoa, ao vivo, com som e um aviso no rodapé.

## O que não entra

Compra, venda, troca, saldo, caixas, raridade sorteada, mercado. Sem isso o back-end não precisa de contabilidade de dinheiro, só de garantias de não duplicar e não perder, que o esquema acima dá com o banco.

## Primeiros passos (quando chegar a hora)

1. Catálogo com 3 itens e o conferidor no CI (F4-01).
2. Ledger, regra de data e `GetInventory`, com testes de idempotência e de transação (F4-02).
3. Visualizador glTF no app, reaproveitando o que o toca-discos já faz (F4-03).
4. Vitrine no perfil (F4-04).
