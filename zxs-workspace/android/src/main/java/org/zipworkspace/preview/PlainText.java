package org.zipworkspace.preview;

import android.icu.lang.UCharacter;
import android.icu.lang.UProperty;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;

/** One literal preview of verified bytes. Larger members yield an explicitly incomplete prefix. */
final class PlainText {
    static final int LIMIT = 256 * 1024;
    static final int PAGE_UNITS = 8 * 1024, PAGE_NEWLINES = 256;
    private PlainText() {}

    static final class Preview {
        final int displayedBytes; // before CRLF presentation normalization
        final long totalBytes;
        private final String text;
        private final ArrayList<Integer> cuts = new ArrayList<>();

        private Preview(String text, int displayedBytes, long totalBytes) {
            this.text = text; this.displayedBytes = displayedBytes; this.totalBytes = totalBytes;
            cuts.add(0);
            int start = 0;
            do {
                int stop = Math.min(text.length(), start + PAGE_UNITS);
                if (stop < text.length() && Character.isLowSurrogate(text.charAt(stop))) stop--;
                for (int i = start, lines = 0; i < stop; i++) {
                    if (text.charAt(i) == '\n' && ++lines == PAGE_NEWLINES) { stop = i + 1; break; }
                }
                cuts.add(stop); start = stop;
            } while (start < text.length());
        }
        boolean complete() { return displayedBytes == totalBytes; }
        int pages() { return cuts.size() - 1; }
        String page(int index) { return text.substring(cuts.get(index), cuts.get(index + 1)); }
    }

    static int captureLength(long total) { return (int)Math.min(total, LIMIT + 3L); }

    static Preview decode(byte[] bytes, long total) {
        if (total < 0 || bytes.length != captureLength(total)) return null;
        int kept = (int)Math.min(total, LIMIT), checked = kept;
        if (total > LIMIT) {
            // Find the final scalar that starts INSIDE the budget. If it is already
            // complete, arbitrary bytes after it are outside this text observation.
            int start = kept - 1, floor = Math.max(0, kept - 4);
            while (start > floor && (bytes[start] & 0xc0) == 0x80) start--;
            int lead = bytes[start] & 0xff;
            int width = lead >= 0xc2 && lead <= 0xdf ? 2 : lead >= 0xe0 && lead <= 0xef ? 3
                    : lead >= 0xf0 && lead <= 0xf4 ? 4 : 1;
            if (width > kept - start) {
                checked = start + width; kept = start; // validate the whole scalar even though it is not shown
                if (checked > bytes.length) return null;
            } else if (bytes[kept - 1] == '\r') {
                // Only omit a CRLF at the edge if the next actual byte completes it.
                if (bytes.length <= kept || bytes[kept] != '\n') return null;
                checked = --kept;
            }
        }
        String text;
        try {
            text = StandardCharsets.UTF_8.newDecoder()
                    .onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT)
                    .decode(ByteBuffer.wrap(bytes, 0, checked)).toString();
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
        if (checked > kept) text = text.substring(0, text.offsetByCodePoints(text.length(), -1));
        return new Preview(text.replace("\r\n", "\n"), kept, total);
    }
}
