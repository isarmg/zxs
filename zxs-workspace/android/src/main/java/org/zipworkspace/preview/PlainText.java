package org.zipworkspace.preview;

import android.icu.lang.UCharacter;
import android.icu.lang.UProperty;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;

/** A deliberately narrow display profile. No MIME sniffing, markup, or replacement decoding. */
final class PlainText {
    static final int LIMIT = 32 * 1024;
    private PlainText() {}

    static String decode(byte[] bytes) {
        if (bytes.length > LIMIT) return null;
        final String text;
        try {
            text = StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(bytes)).toString();
        } catch (CharacterCodingException invalid) { return null; }
        for (int i = 0; i < text.length();) {
            int cp = text.codePointAt(i);
            i += Character.charCount(cp);
            if (cp == '\t' || cp == '\n') continue;
            if (cp == '\r') {
                if (i >= text.length() || text.charAt(i++) != '\n') return null;
                continue;
            }
            int type = Character.getType(cp);
            if (type == Character.CONTROL || type == Character.FORMAT || type == Character.LINE_SEPARATOR
                    || type == Character.PARAGRAPH_SEPARATOR || type == Character.UNASSIGNED || type == Character.PRIVATE_USE
                    || UCharacter.hasBinaryProperty(cp, UProperty.DEFAULT_IGNORABLE_CODE_POINT)) return null;
        }
        return text.replace("\r\n", "\n"); // only presentation changes; the CRC covers the original bytes
    }
}
