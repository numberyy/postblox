-- Slice 7: drafts now carry the RFC 5322 threading headers (In-Reply-To,
-- References) so message.send can stitch a reply onto the original
-- thread. Both columns are NULL for non-reply drafts.
ALTER TABLE drafts ADD COLUMN in_reply_to TEXT;
ALTER TABLE drafts ADD COLUMN references_header TEXT;
