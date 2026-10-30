-- Primeira versão do banco: pessoas, conversas, quem participa de cada uma
-- e as mensagens.

-- Sem login ainda: a pessoa é identificada só pelo nome de usuário.
CREATE TABLE users (
    id         UUID PRIMARY KEY,
    username   TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE conversations (
    id         UUID PRIMARY KEY,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('direct', 'group')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Quem participa de cada conversa. Só os membros recebem as mensagens.
CREATE TABLE conversation_members (
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    joined_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, user_id)
);

-- Para achar rápido as conversas de uma pessoa.
CREATE INDEX conversation_members_by_user ON conversation_members (user_id);

CREATE TABLE messages (
    id              BIGSERIAL PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    author_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    text            TEXT NOT NULL,
    -- Milissegundos desde 1970, o mesmo formato do protocolo.
    sent_at_ms      BIGINT NOT NULL
);

-- O histórico e a última mensagem são sempre buscados por conversa e data.
CREATE INDEX messages_by_conversation ON messages (conversation_id, sent_at_ms);

-- O grupo em que toda pessoa nova entra, com id fixo para o servidor achar.
INSERT INTO conversations (id, name, kind)
VALUES ('00000000-0000-0000-0000-000000000001', 'Sala do Grupo', 'group');
