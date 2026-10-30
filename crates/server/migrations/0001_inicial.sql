-- Primeiras tabelas do mothh: usuários, sessões, conversas e mensagens.

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username TEXT NOT NULL,
    -- Fica vazio até o login de verdade existir.
    password_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Nomes únicos sem diferenciar maiúsculas: "Lia" e "lia" são a mesma pessoa.
CREATE UNIQUE INDEX users_username_key ON users (lower(username));

-- Guarda só o hash do token, nunca o token em si.
CREATE TABLE sessions (
    token_hash BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE conversations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('direct', 'group')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE conversation_members (
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, user_id)
);

CREATE INDEX conversation_members_user ON conversation_members (user_id);

CREATE TABLE messages (
    -- Número sequencial: ordena as mensagens sem depender do relógio.
    id BIGSERIAL PRIMARY KEY,
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    author_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    sent_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX messages_conversation_recent ON messages (conversation_id, id DESC);

-- A sala onde todo mundo entra. O id fixo é o `GROUP_ROOM` do código.
INSERT INTO conversations (id, name, kind)
VALUES ('00000000-0000-0000-0000-000000000001', 'Sala do Grupo', 'group');
