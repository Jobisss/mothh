-- Vários aparelhos por conta. Cada instalação do app entrada numa conta é
-- um aparelho, e cada aparelho é um membro próprio dos grupos MLS: o que um
-- manda, os outros da mesma conta leem.
--
-- O aparelho novo de uma conta que já tem outro espera ser aprovado nele
-- (`approved` falso, com um `code` para conferir nos dois). O primeiro
-- aparelho de cada conta entra aprovado.
CREATE TABLE devices (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- O id aleatório que a instalação guarda (`ClientMessage::Hello`).
    install UUID NOT NULL,
    name TEXT NOT NULL,
    approved BOOLEAN NOT NULL DEFAULT false,
    code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, install)
);

-- O aparelho de cada sessão. Vazio numa sessão de antes desta migração: o
-- próximo `Resume` preenche. Tirar o aparelho derruba as sessões dele.
ALTER TABLE sessions ADD COLUMN device_id UUID REFERENCES devices (id) ON DELETE CASCADE;

-- Os grupos MLS passam a ter aparelhos, não pessoas. Os grupos de antes
-- (uma folha por pessoa) não servem mais: todos recomeçam. O texto já lido
-- continua no app de cada um (a cópia local); o servidor nunca teve o texto.
DROP TABLE mls_members;
DROP TABLE mls_welcomes;
DROP TABLE key_packages;
DELETE FROM mls_commits;
UPDATE conversations SET mls_epoch = 0;

-- Sem chave estrangeira para `devices`: um aparelho tirado da conta continua
-- aqui até alguém tirá-lo da árvore do MLS.
CREATE TABLE mls_members (
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    device_id UUID NOT NULL,
    PRIMARY KEY (conversation_id, device_id)
);

CREATE TABLE key_packages (
    id BIGSERIAL PRIMARY KEY,
    device_id UUID NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    data BYTEA NOT NULL
);

CREATE INDEX key_packages_device ON key_packages (device_id, id);

CREATE TABLE mls_welcomes (
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    epoch BIGINT NOT NULL,
    data BYTEA NOT NULL,
    PRIMARY KEY (conversation_id, device_id)
);
