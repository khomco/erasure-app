-- Notes on sales leads (lead-capture story 2, CAP-3): vendor staff's working
-- notes, written in the admin area. A note is append-only: there is no edit
-- or delete of a single note. It is attributed to the vendor admin who wrote
-- it; removing that user keeps the note with no author (`ON DELETE SET
-- NULL`). Erasing the lead deletes its notes (`ON DELETE CASCADE`), like its
-- submissions, so an erasure leaves no note behind (CAP-4).
--
-- Never edit an applied migration; add a new NNNN_name.sql.

CREATE TABLE lead_notes (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    lead_id    uuid NOT NULL REFERENCES leads (id) ON DELETE CASCADE,
    author_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    body       text NOT NULL CHECK (body ~ '\S'),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX lead_notes_lead_id_idx ON lead_notes (lead_id);
