/**
 * gaia's `values.language` is a GRC-20 language entity id, derived from an ISO 639-1 code as the Geo genesis
 * Space defines it (grc-20 `genesis::language_id`): the first 16 bytes of
 * SHA-256("grc20:genesis:language:" + code), version nibble 8, RFC 4122 variant. The testnet database holds one
 * language id, English (090adac0-fca4-822e-8e71-9263e67620ec), which this derivation reproduces.
 *
 * geo-db wants a canonical lowercase BCP 47 tag, so each id maps to its primary subtag. An id that matches no
 * ISO 639-1 code has no tag and the caller keeps the plain text.
 */
import {createHash} from "node:crypto"
import {hexId} from "./ids.js"

/** The 184 ISO 639-1 codes. */
const ISO_639_1 = (
	"aa ab ae af ak am an ar as av ay az ba be bg bh bi bm bn bo br bs ca ce ch co cr cs cu cv cy da de dv dz ee el " +
	"en eo es et eu fa ff fi fj fo fr fy ga gd gl gn gu gv ha he hi ho hr ht hu hy hz ia id ie ig ii ik io is it iu " +
	"ja jv ka kg ki kj kk kl km kn ko kr ks ku kv kw ky la lb lg li ln lo lt lu lv mg mh mi mk ml mn mr ms mt my na " +
	"nb nd ne ng nl nn no nr nv ny oc oj om or os pa pi pl ps pt qu rm rn ro ru rw sa sc sd se sg si sk sl sm sn so " +
	"sq sr ss st su sv sw ta te tg th ti tk tl tn to tr ts tt tw ty ug uk ur uz ve vi vo wa wo xh yi yo za zh zu"
).split(" ")

/** The id of the Geo genesis language entity for `code`, as 32 lowercase hex digits. */
export function genesisLanguageId(code: string): string {
	const bytes = createHash("sha256").update(`grc20:genesis:language:${code}`).digest().subarray(0, 16)
	bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x80
	bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80
	return bytes.toString("hex")
}

const TAG_OF_ID = new Map<string, string>(ISO_639_1.map((code) => [genesisLanguageId(code), code]))

/** The BCP 47 primary subtag of a gaia language id (dashed or compact, string or bytes), or null if unknown. */
export function languageTag(language: string | Uint8Array | null | undefined): string | null {
	if (language == null) return null
	return TAG_OF_ID.get(hexId(language).replaceAll("-", "").toLowerCase()) ?? null
}
