package org.zipworkspace.preview;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Context;
import android.graphics.Insets;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import java.io.IOException;

/** Read-only reopening of snapshots in the application's own store. */
public final class LocalSnapshotsActivity extends Activity {
    private final Handler main = new Handler(Looper.getMainLooper());
    private LinearLayout rows;
    private TextView status;
    private AlertDialog opened;
    private Object request;
    private boolean active;

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        ScrollView scroll = new ScrollView(this);
        LinearLayout body = new LinearLayout(this);
        body.setOrientation(LinearLayout.VERTICAL);
        int gap = (int)(12 * getResources().getDisplayMetrics().density);
        body.setOnApplyWindowInsetsListener((view, win) -> {
            Insets bars = win.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            view.setPadding(bars.left + gap, bars.top + gap, bars.right + gap, bars.bottom + gap);
            return win;
        });
        TextView title = new TextView(this);
        title.setText("本机已提交快照");
        title.setTextSize(22);
        title.setAccessibilityHeading(true);
        body.addView(title);
        status = new TextView(this);
        status.setAccessibilityLiveRegion(android.view.View.ACCESSIBILITY_LIVE_REGION_POLITE);
        body.addView(status);
        rows = new LinearLayout(this);
        rows.setOrientation(LinearLayout.VERTICAL);
        body.addView(rows);
        scroll.addView(body);
        setContentView(scroll);
    }

    @Override protected void onStart() {
        super.onStart();
        active = true;
        list(null);
    }

    @Override protected void onStop() {
        active = false;
        request = null;
        if (opened != null) opened.dismiss();
        rows.removeAllViews();
        super.onStop();
    }

    private interface Read<T> { T run(Context context) throws IOException; }

    private <T> void read(Read<T> action, java.util.function.Consumer<T> deliver) {
        Object token = new Object();
        request = token;
        Context app = getApplicationContext();
        new Thread(() -> {
            T result;
            try { result = action.run(app); }
            catch (IOException | SecurityException | java.nio.file.DirectoryIteratorException failure) { result = null; }
            T ready = result;
            main.post(() -> {
                if (active && request == token && !isFinishing() && !isDestroyed()) {
                    request = null;
                    deliver.accept(ready);
                }
            });
        }, "local-snapshot-read").start();
    }

    private void button(String title, Runnable action) {
        Button button = new Button(this);
        button.setText(title);
        button.setAllCaps(false);
        button.setOnClickListener(ignored -> {
            if (active && request == null && opened == null && button.getParent() == rows) action.run();
        });
        rows.addView(button);
    }

    private void list(String after) {
        if (!active || request != null) return;
        rows.removeAllViews();
        status.setText("正在读取本机快照…");
        read(app -> LocalWorkspaceStore.published(app, after), page -> {
            if (page == null) {
                status.setText("无法读取本机快照。");
                button("重试", () -> list(after));
                return;
            }
            status.setText(page.names().isEmpty() ? "没有找到本机快照。" : "选择要只读打开的快照。");
            if (after != null) button("回到第一页", () -> list(null));
            for (String name : page.names()) button(name, () -> reopen(name));
            if (page.next() != null) button("下一页", () -> list(page.next()));
        });
    }

    private void reopen(String name) {
        status.setText("正在核验并打开快照…");
        read(app -> LocalWorkspaceStore.reopen(app, name), snapshot -> {
            if (snapshot == null) {
                status.setText("无法核验这份快照。");
                return;
            }
            status.setText("选择要只读打开的快照。");
            show(snapshot);
        });
    }

    private void show(LocalWorkspaceStore.Reopened snapshot) {
        PlainText.Preview preview = snapshot.text();
        int[] page = {0};
        AlertDialog dialog = new AlertDialog.Builder(this).setTitle(snapshot.name()).setMessage("")
                .setNegativeButton("上一页", null).setNeutralButton("下一页", null)
                .setPositiveButton("关闭", null).create();
        opened = dialog;
        dialog.setOnDismissListener(ignored -> { if (opened == dialog) opened = null; });
        dialog.show();
        Runnable render = () -> {
            String body = preview.totalBytes == 0 ? "（空文本）" : preview.page(page[0]);
            dialog.setMessage((page[0] + 1) + " / " + preview.pages() + "\n\n" + body);
            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).setEnabled(page[0] > 0);
            dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setEnabled(page[0] + 1 < preview.pages());
        };
        dialog.getButton(AlertDialog.BUTTON_NEGATIVE).setOnClickListener(v -> {
            if (page[0] > 0) { page[0]--; render.run(); }
        });
        dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(v -> {
            if (page[0] + 1 < preview.pages()) { page[0]++; render.run(); }
        });
        render.run();
    }
}
