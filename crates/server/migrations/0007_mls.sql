-- F2-08: o texto das mensagens passa a chegar cifrado com MLS. O servidor
-- guarda só os bytes cifrados (`sealed`) e nunca vê o texto.
--
-- As mensagens de texto antigas estavam em claro e não têm como ser cifradas
-- aqui (o servidor não tem as chaves de ninguém): saem. Áudios e arquivos
-- ficam.
DELETE FROM messages WHERE attachment_id IS NULL;
ALTER TABLE messages DROP COLUMN text;
ALTER TABLE messages ADD COLUMN sealed BYTEA;

-- Época atual do grupo MLS da conversa. Cada commit aceito soma um; com
-- `mls_members` vazio, o grupo ainda não existe.
ALTER TABLE conversations ADD COLUMN mls_epoch BIGINT NOT NULL DEFAULT 0;

-- Quem está no grupo MLS, segundo os commits aceitos. Pode diferir de
-- `conversation_members` por um instante: a diferença é o que falta fazer.
CREATE TABLE mls_members (
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, user_id)
);

-- Pacotes de chaves publicados por cada pessoa; cada um é entregue uma vez.
CREATE TABLE key_packages (
    id BIGSERIAL PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    data BYTEA NOT NULL
);

CREATE INDEX key_packages_user ON key_packages (user_id, id);

-- Commits aceitos, para quem ficou offline alcançar a época atual.
CREATE TABLE mls_commits (
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    epoch BIGINT NOT NULL,
    data BYTEA NOT NULL,
    PRIMARY KEY (conversation_id, epoch)
);

-- O welcome mais recente de cada pessoa em cada conversa, até ela usar.
CREATE TABLE mls_welcomes (
    conversation_id UUID NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    epoch BIGINT NOT NULL,
    data BYTEA NOT NULL,
    PRIMARY KEY (conversation_id, user_id)
);
