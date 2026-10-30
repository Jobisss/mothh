-- Anexos enviados por HTTP. O arquivo fica fora do banco (pasta local por
-- enquanto); aqui ficam quem enviou, o tipo e, no áudio, a duração.
CREATE TABLE attachments (
    id UUID PRIMARY KEY,
    uploader_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    content_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    -- Só áudio tem duração.
    duration_ms INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Uma mensagem de áudio tem texto vazio e aponta para o anexo.
ALTER TABLE messages ADD COLUMN attachment_id UUID REFERENCES attachments (id);

CREATE INDEX messages_attachment ON messages (attachment_id) WHERE attachment_id IS NOT NULL;
