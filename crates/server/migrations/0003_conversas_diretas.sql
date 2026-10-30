-- Conversas diretas: no máximo uma por par de pessoas.
-- O par é guardado em ordem (menor id primeiro), assim (a, b) e (b, a) caem na
-- mesma chave primária e o banco recusa a segunda.
CREATE TABLE direct_conversations (
    user_low UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    user_high UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL UNIQUE REFERENCES conversations (id) ON DELETE CASCADE,
    PRIMARY KEY (user_low, user_high),
    CHECK (user_low < user_high)
);
