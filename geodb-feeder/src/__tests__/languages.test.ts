import {describe, expect, it} from "vitest"
import {genesisLanguageId, languageTag} from "../languages.js"

describe("languageTag", () => {
	it("derives the genesis ids: English is the one id the testnet holds", () => {
		expect(genesisLanguageId("en")).toBe("090adac0fca4822e8e719263e67620ec")
	})

	it("maps an id, dashed or compact, in any case, string or bytes, to its primary subtag", () => {
		expect(languageTag("090adac0-fca4-822e-8e71-9263e67620ec")).toBe("en")
		expect(languageTag("090ADAC0FCA4822E8E719263E67620EC")).toBe("en")
		expect(languageTag(Uint8Array.from(Buffer.from(genesisLanguageId("de"), "hex")))).toBe("de")
		expect(languageTag(genesisLanguageId("zh"))).toBe("zh")
	})

	it("has no tag for an unknown id or none", () => {
		expect(languageTag("00000000-0000-8000-8000-000000000000")).toBeNull()
		expect(languageTag(null)).toBeNull()
		expect(languageTag(undefined)).toBeNull()
	})

	it("derives a distinct id for each of the 184 codes", () => {
		const codes = "aa ab ae af ak am an ar as av ay az ba be bg bh bi bm bn bo br bs ca ce ch co cr cs cu cv cy da de dv dz ee el en eo es et eu fa ff fi fj fo fr fy ga gd gl gn gu gv ha he hi ho hr ht hu hy hz ia id ie ig ii ik io is it iu ja jv ka kg ki kj kk kl km kn ko kr ks ku kv kw ky la lb lg li ln lo lt lu lv mg mh mi mk ml mn mr ms mt my na nb nd ne ng nl nn no nr nv ny oc oj om or os pa pi pl ps pt qu rm rn ro ru rw sa sc sd se sg si sk sl sm sn so sq sr ss st su sv sw ta te tg th ti tk tl tn to tr ts tt tw ty ug uk ur uz ve vi vo wa wo xh yi yo za zh zu".split(" ")
		expect(new Set(codes.map(genesisLanguageId)).size).toBe(codes.length)
		for (const c of codes) expect(languageTag(genesisLanguageId(c))).toBe(c)
	})
})
