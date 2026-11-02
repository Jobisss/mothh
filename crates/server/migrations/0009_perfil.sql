-- Página de perfil: os textos e as vitrines de cada pessoa. A versão sobe a
-- cada mudança (texto ou imagem), para os apps saberem que a página mudou.
CREATE TABLE profiles (
    user_id UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    real_name TEXT NOT NULL DEFAULT '',
    location TEXT NOT NULL DEFAULT '',
    about TEXT NOT NULL DEFAULT '',
    accent TEXT NOT NULL DEFAULT '',
    showcases TEXT[] NOT NULL DEFAULT '{art,about}',
    version BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- As imagens da página (foto grande, fundo, artes). O arquivo fica na pasta
-- dos anexos, em perfil/<id da pessoa>-<tipo>; aqui só o tipo e a versão.
CREATE TABLE profile_images (
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    content_type TEXT NOT NULL,
    version BIGINT NOT NULL,
    PRIMARY KEY (user_id, kind)
);
