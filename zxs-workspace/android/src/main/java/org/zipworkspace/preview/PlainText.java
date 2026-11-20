package org.zipworkspace.preview;

import android.icu.lang.UCharacter;
import android.icu.lang.UProperty;
import java.nio.ByteBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.CharsetDecoder;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.function.IntConsumer;

/** Literal, non-interactive pages under a single UTF-8 display profile. */
final class PlainText {
    static final int LIMIT = 256 * 1024;
    static final int PAGE_UNITS = 8 * 1024, PAGE_NEWLINES = 256;
    private PlainText() {}

    static final class Preview {
        final int displayedBytes; // before CRLF presentation normalization
        final long totalBytes;
        private final String text;
        private final ArrayList<Integer> cuts;

        private Preview(String text, int displayedBytes, long totalBytes, ArrayList<Integer> cuts) {
            this.text = text; this.displayedBytes = displayedBytes; this.totalBytes = totalBytes; this.cuts = cuts;
        }
        boolean complete() { return displayedBytes == totalBytes; }
        int pages() { return cuts.size() - 1; }
        String page(int index) { return literalPage(text, cuts.get(index), cuts.get(index + 1)); }
    }

    static int captureLength(long total) { return (int)Math.min(total, LIMIT + 3L); }

    static CharsetDecoder decoder() {
        return StandardCharsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT);
    }

    /** Bytes at the end of a nonfinal block that may start a crossing scalar or CRLF. */
    static int trailingUnit(byte[] bytes, int length) {
        if (length == 0) return 0;
        if (bytes[length - 1] == '\r') return 1;
        int start = length - 1, floor = Math.max(0, length - 4);
        while (start > floor && (bytes[start] & 0xc0) == 0x80) start--;
        return scalarWidth(bytes[start]) > length - start ? length - start : 0;
    }

    private static int scalarWidth(byte first) {
        int lead = first & 0xff;
        return lead >= 0xc2 && lead <= 0xdf ? 2 : lead >= 0xe0 && lead <= 0xef ? 3
                : lead >= 0xf0 && lead <= 0xf4 ? 4 : 1;
    }

    private static boolean permitted(int cp) {
        if ((cp >= 0x20 && cp <= 0x7e) || cp == '\t' || cp == '\n') return true;
        int type = Character.getType(cp);
        return type != Character.CONTROL && type != Character.FORMAT && type != Character.LINE_SEPARATOR
                && type != Character.PARAGRAPH_SEPARATOR && type != Character.UNASSIGNED
                && type != Character.PRIVATE_USE && type != Character.SURROGATE
                && !UCharacter.hasBinaryProperty(cp, UProperty.DEFAULT_IGNORABLE_CODE_POINT);
    }

    /** Validate and paginate the decoded characters. No per-page table is needed by a counting caller. */
    static int scanPages(CharSequence text, Runnable check, IntConsumer pageEnd) {
        int stop = 0, count = 0;
        do {
            check.run();
            for (int units = 0, lines = 0; stop < text.length() && units < PAGE_UNITS && lines < PAGE_NEWLINES;) {
                int cp = Character.codePointAt(text, stop);
                int rawUnits = Character.charCount(cp);
                if (cp == '\r') {
                    if (stop + 1 >= text.length() || text.charAt(stop + 1) != '\n') return -1;
                    cp = '\n'; rawUnits = 2;
                }
                if (!permitted(cp)) return -1;
                int shown = Character.charCount(cp);
                if (units + shown > PAGE_UNITS) break;
                if (cp == '\n') lines++;
                stop += rawUnits; units += shown;
            }
            if (pageEnd != null) pageEnd.accept(stop);
            count++;
        } while (stop < text.length());
        return count;
    }

    static String literalPage(CharSequence text, int start, int end) {
        return text.subSequence(start, end).toString().replace("\r\n", "\n");
    }

    static Preview decode(byte[] bytes, long total, Runnable check) {
        if (total < 0 || bytes.length != captureLength(total)) return null;
        int kept = (int)Math.min(total, LIMIT), checked = kept;
        if (total > LIMIT) {
            int crossing = trailingUnit(bytes, kept);
            if (crossing != 0) {
                if (bytes[kept - 1] == '\r') {
                    // Only omit a CRLF if the next actual byte completes it.
                    if (bytes.length <= kept || bytes[kept] != '\n') return null;
                    checked = --kept;
                } else {
                    kept -= crossing; checked = kept + scalarWidth(bytes[kept]);
                    if (checked > bytes.length) return null;
                }
            }
        }
        String text;
        try { text = decoder().decode(ByteBuffer.wrap(bytes, 0, checked)).toString(); }
        catch (CharacterCodingException invalid) { return null; }
        if (checked > kept) {
            int cp = text.codePointBefore(text.length()); // crossing scalar already decoded, never silently discarded if disallowed
            if (!permitted(cp)) return null;
            text = text.substring(0, text.length() - Character.charCount(cp));
        }
        ArrayList<Integer> cuts = new ArrayList<>(); cuts.add(0);
        return scanPages(text, check, cuts::add) < 0 ? null : new Preview(text, kept, total, cuts);
    }
}
