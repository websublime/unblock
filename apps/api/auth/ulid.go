// Minimal ULID generator (per the ULID spec
// https://github.com/ulid/spec). The auth service mints ULIDs for
// `mcp.api_keys.id`, `auth.users.id`, `auth.oauth_tokens.id`, and
// `auth.sessions.id` (SPEC §4.1, §4.3.2).
//
// Why an inline implementation: the apps/api Go module currently has
// zero third-party dependencies (`go.mod` lists only encore.dev). Pulling
// `github.com/oklog/ulid/v2` for a 50-line generator would expand the
// module's dependency surface and complicate Olive's vendoring story for
// little benefit. The ULID spec is small enough to implement directly,
// and crypto/rand + 48-bit ms timestamp is the entire algorithm.
//
// Format (locked by the ULID spec):
//
//	48 bits  Unix milliseconds (big-endian)
//	80 bits  crypto/rand entropy
//	------
//	128 bits total, encoded as 26 chars Crockford base32 (uppercase).

package auth

import (
	"crypto/rand"
	"fmt"
	"time"
)

// crockfordAlphabet is Crockford base32 (RFC 4648 alphabet minus
// I, L, O, U). Uppercase per the ULID spec.
const crockfordAlphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"

// newULID returns a 26-char Crockford-base32 ULID. The ms-precision
// timestamp anchors the lexicographic order; the random tail provides
// 80 bits of entropy per millisecond (sufficient for the issuance
// rates expected in P01: human-driven seeder calls and operator
// surfaces).
//
// crypto/rand is the entropy source. Returns a structured error on
// rand.Read failure (extremely rare but propagated rather than
// panicked so RPC handlers can return errs.Internal).
func newULID() (string, error) {
	var buf [16]byte

	// First 6 bytes: ms timestamp big-endian.
	ms := uint64(time.Now().UnixMilli())
	buf[0] = byte(ms >> 40)
	buf[1] = byte(ms >> 32)
	buf[2] = byte(ms >> 24)
	buf[3] = byte(ms >> 16)
	buf[4] = byte(ms >> 8)
	buf[5] = byte(ms)

	// Last 10 bytes: crypto/rand entropy.
	if _, err := rand.Read(buf[6:]); err != nil {
		return "", fmt.Errorf("auth: ulid entropy: %w", err)
	}

	// Encode the 16-byte value as 26 chars of Crockford base32. The
	// ULID spec packs 130 bits (5 bits per char * 26 chars) into the
	// 128 data bits with the leading char's top 3 bits as zero. We
	// implement the canonical spec encoding directly.
	var out [26]byte
	out[0] = crockfordAlphabet[(buf[0]&224)>>5]
	out[1] = crockfordAlphabet[buf[0]&31]
	out[2] = crockfordAlphabet[(buf[1]&248)>>3]
	out[3] = crockfordAlphabet[((buf[1]&7)<<2)|((buf[2]&192)>>6)]
	out[4] = crockfordAlphabet[(buf[2]&62)>>1]
	out[5] = crockfordAlphabet[((buf[2]&1)<<4)|((buf[3]&240)>>4)]
	out[6] = crockfordAlphabet[((buf[3]&15)<<1)|((buf[4]&128)>>7)]
	out[7] = crockfordAlphabet[(buf[4]&124)>>2]
	out[8] = crockfordAlphabet[((buf[4]&3)<<3)|((buf[5]&224)>>5)]
	out[9] = crockfordAlphabet[buf[5]&31]
	out[10] = crockfordAlphabet[(buf[6]&248)>>3]
	out[11] = crockfordAlphabet[((buf[6]&7)<<2)|((buf[7]&192)>>6)]
	out[12] = crockfordAlphabet[(buf[7]&62)>>1]
	out[13] = crockfordAlphabet[((buf[7]&1)<<4)|((buf[8]&240)>>4)]
	out[14] = crockfordAlphabet[((buf[8]&15)<<1)|((buf[9]&128)>>7)]
	out[15] = crockfordAlphabet[(buf[9]&124)>>2]
	out[16] = crockfordAlphabet[((buf[9]&3)<<3)|((buf[10]&224)>>5)]
	out[17] = crockfordAlphabet[buf[10]&31]
	out[18] = crockfordAlphabet[(buf[11]&248)>>3]
	out[19] = crockfordAlphabet[((buf[11]&7)<<2)|((buf[12]&192)>>6)]
	out[20] = crockfordAlphabet[(buf[12]&62)>>1]
	out[21] = crockfordAlphabet[((buf[12]&1)<<4)|((buf[13]&240)>>4)]
	out[22] = crockfordAlphabet[((buf[13]&15)<<1)|((buf[14]&128)>>7)]
	out[23] = crockfordAlphabet[(buf[14]&124)>>2]
	out[24] = crockfordAlphabet[((buf[14]&3)<<3)|((buf[15]&224)>>5)]
	out[25] = crockfordAlphabet[buf[15]&31]

	return string(out[:]), nil
}
