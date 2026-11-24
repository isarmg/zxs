package org.zipworkspace.preview;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Context;
import android.graphics.Insets;
import android.os.Bundle;
import android.text.InputFilter;
import android.text.InputType;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.IOException;
import java.math.BigInteger;
import java.nio.CharBuffer;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.regex.Pattern;

/** Explicit first whole-text save and original-operation recovery. */
public final class RemoteSaveActivity extends Activity {
    private static final Pattern OPERATION = Pattern.compile("[0-9a-f]{32}");
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private EditText origin, account, password, name, body, operation;
    private Button create, recover;
    private TextView status;
    private Future<?> pending;
    private AlertDialog decision;
    private RemoteH2Exchange peer;
    private boolean running, live, lockedNew;
    private int epoch;

    @Override protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        ScrollView scroll = new ScrollView(this);
        LinearLayout form = new LinearLayout(this);
        form.setOrientation(LinearLayout.VERTICAL);
        form.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS);
        int gap = (int) (12 * getResources().getDisplayMetrics().density);
        form.setOnApplyWindowInsetsListener((view, win) -> {
            Insets bars = win.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            view.setPadding(bars.left + gap, bars.top + gap, bars.right + gap, bars.bottom + gap);
            return win;
        });
        scroll.addView(form);
        TextView title = new TextView(this);
        title.setText("远程文件");
        title.setAccessibilityHeading(true);
        title.setTextSize(22);
        form.addView(title);
        origin = field(form, "服务器 HTTPS 地址", InputType.TYPE_TEXT_VARIATION_URI, 1024, true);
        account = field(form, "账号", InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD, 64, true);
        password = field(form, "密码", InputType.TYPE_TEXT_VARIATION_PASSWORD, 1024, true);
        status = new TextView(this);
        status.setTextIsSelectable(true);
        status.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        form.addView(status);
        operation = field(form, "原操作号", InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD, 32, true);
        recover = button(form, "查看原提交…", this::loadOriginal);
        name = field(form, "新文件名", InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS, 255, true);
        body = field(form, "全文", InputType.TYPE_TEXT_FLAG_MULTI_LINE, PlainText.LIMIT, false);
        body.setMinLines(6);
        create = button(form, "提交新文件…", this::firstDecision);
        if (saved != null) {
            operation.setText(saved.getString("operation", ""));
            lockedNew = saved.getBoolean("original_frozen");
        }
        setContentView(scroll);
        showControls();
    }

    private EditText field(LinearLayout form, String hint, int type, int limit, boolean singleLine) {
        EditText field = new EditText(this);
        field.setHint(hint);
        field.setSingleLine(singleLine);
        field.setInputType(InputType.TYPE_CLASS_TEXT | type);
        field.setImeOptions(EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING);
        field.setFilters(new InputFilter[]{new InputFilter.LengthFilter(limit)});
        field.setSaveEnabled(false);
        form.addView(field);
        return field;
    }

    private Button button(LinearLayout form, String label, Runnable click) {
        Button button = new Button(this);
        button.setText(label);
        button.setOnClickListener(v -> click.run());
        form.addView(button);
        return button;
    }

    private void firstDecision() {
        if (!live || running || decision != null || lockedNew) return;
        byte[] raw;
        RemoteSaveContract.Save save;
        try {
            var target = new RemoteSaveContract.FirstMain(name.getText().toString());
            String text = body.getText().toString();
            if (PlainText.scanPages(text, () -> {}, null) < 0) throw new IOException("unsupported text");
            var encoded = StandardCharsets.UTF_8.newEncoder().encode(CharBuffer.wrap(text));
            if (encoded.remaining() > PlainText.LIMIT) throw new IOException("too long");
            raw = new byte[encoded.remaining()];
            encoded.get(raw);
            save = new RemoteSaveContract.Save(id(), id(), id(), BigInteger.ZERO, target, raw.length,
                    ArchiveIndex.hex(ArchiveIndex.sha256().digest(raw)));
        } catch (IllegalArgumentException | IOException invalid) {
            status.setText("请检查文件名、文本格式和长度。");
            return;
        }

        String selected = origin.getText().toString();
        var exactPeer = takePeer(selected);
        if (exactPeer == null) {
            Arrays.fill(raw, (byte) 0);
            return;
        }
        var plan = new RemoteSaveJournal.Plan(exactPeer.authority(), save);
        decision = new AlertDialog.Builder(this).setTitle("确认提交完整文件")
                .setMessage(summary(selected, plan))
                .setNegativeButton("取消", null)
                .setPositiveButton("提交一次", (dialog, which) -> {
                    if (!owns(exactPeer) || lockedNew) return;
                    lockedNew = true;
                    operation.setText(save.operation());
                    status.setText("正在准备并提交：\n" + save.operation());
                    runOperation(plan, exactPeer, RemoteSaveSubmit.Mode.FIRST_SEND, raw.clone());
                }).create();
        decision.setOnDismissListener(dialog -> {
            decision = null;
            Arrays.fill(raw, (byte) 0);
            if (!running) releasePeer(exactPeer);
        });
        decision.show();
    }

    private void loadOriginal() {
        if (!live || running || decision != null) return;
        String op = operation.getText().toString();
        if (!OPERATION.matcher(op).matches() || "0".repeat(32).equals(op)) {
            operation.setError("请输入原操作号。");
            return;
        }
        String selected = origin.getText().toString();
        var exactPeer = takePeer(selected);
        if (exactPeer == null) return;
        Context context = getApplicationContext();
        int current = beginTask();
        pending = worker.submit(() -> {
            RemoteSaveJournal.Plan loaded;
            boolean possiblySent;
            try {
                var attempted = RemoteSaveJournal.attempted(context, exactPeer.authority(), op);
                possiblySent = attempted != null;
                var retained = possiblySent ? null : RemoteSaveJournal.reopen(context, exactPeer.authority(), op);
                loaded = possiblySent ? attempted
                        : new RemoteSaveJournal.Plan(retained.authority(), retained.save());
            } catch (IOException | RuntimeException invalid) {
                loaded = null;
                possiblySent = false;
            }
            var original = loaded;
            boolean attempted = possiblySent;
            runOnUiThread(() -> {
                if (!finishTask(current, exactPeer)) return;
                if (original == null) {
                    status.setText("无法核验本机原提交；未发出请求。\n" + op);
                    releasePeer(exactPeer);
                } else {
                    confirmOriginal(selected, original, attempted, exactPeer);
                }
            });
        });
    }

    private void confirmOriginal(String selected, RemoteSaveJournal.Plan plan, boolean attempted,
                                 RemoteH2Exchange exactPeer) {
        if (!owns(exactPeer)) return;
        String wording = attempted ? "确认同号重发" : "首次提交";
        var mode = attempted ? RemoteSaveSubmit.Mode.RETRY_SAME_OPERATION : RemoteSaveSubmit.Mode.FIRST_SEND;
        decision = new AlertDialog.Builder(this).setTitle("原提交")
                .setMessage(summary(selected, plan))
                .setNegativeButton("取消", null)
                .setNeutralButton("只查回执", (dialog, which) ->
                        runOperation(plan, exactPeer, RemoteSaveSubmit.Mode.LOOKUP_ONLY, null))
                .setPositiveButton(wording, (dialog, which) -> runOperation(plan, exactPeer, mode, null))
                .create();
        decision.setOnDismissListener(dialog -> {
            decision = null;
            if (!running) releasePeer(exactPeer);
        });
        decision.show();
    }

    private void runOperation(RemoteSaveJournal.Plan plan, RemoteH2Exchange exactPeer,
                              RemoteSaveSubmit.Mode mode, byte[] firstBody) {
        if (!owns(exactPeer) || running) {
            if (firstBody != null) Arrays.fill(firstBody, (byte) 0);
            return;
        }
        int current = beginTask();
        Context context = getApplicationContext();
        pending = worker.submit(() -> {
            RemoteSaveSubmit.Result result;
            try {
                if (firstBody != null) {
                    RemoteSaveJournal.prepare(context, plan.authority(), plan.save(), firstBody);
                }
                result = RemoteSaveSubmit.execute(context, plan, mode, exactPeer);
            } catch (IOException | RuntimeException refused) {
                result = new RemoteSaveSubmit.Result(RemoteSaveSubmit.Status.LOCAL_UNAVAILABLE, null);
            } finally {
                if (firstBody != null) Arrays.fill(firstBody, (byte) 0);
                exactPeer.close();
            }
            var delivered = result;
            runOnUiThread(() -> {
                if (!finishTask(current, exactPeer)) return;
                releasePeer(exactPeer);
                status.setText(resultText(delivered) + "\n" + plan.save().operation());
            });
        });
    }

    private static String summary(String selected, RemoteSaveJournal.Plan plan) {
        var save = plan.save();
        String title = save.target() instanceof RemoteSaveContract.FirstMain first ? first.name() : save.container();
        String base = save.target() instanceof RemoteSaveContract.BranchHead head ? head.expectedHead()
                : save.target() instanceof RemoteSaveContract.HistoricalMain fork ? fork.fromNode() : "首个节点";
        return "服务器：" + selected + "\n账号：" + plan.authority().account() + "\n文件：" + title
                + "\n分支：" + save.target().branch() + "\n基点：" + base + "\n新节点：" + save.node()
                + "\n完整长度：" + save.length() + " 字节"
                + "\nSHA-256：" + save.sha256() + "\n原操作号：" + save.operation();
    }

    private static String resultText(RemoteSaveSubmit.Result result) {
        return switch (result.status()) {
            case CONFIRMED -> "已确认提交。版本 " + result.sequence();
            case ABSENT -> "未查到原回执。未发送正文。";
            case RETRY_REQUIRES_CONFIRMATION -> "未查到原回执。再次发送须确认同一个操作号。";
            case CONFLICT -> "服务端报告冲突。已保留本机原提交。";
            case UNCONFIRMED -> "结果未确认。可用原操作号重查。";
            case LOCAL_UNAVAILABLE -> "无法核验本机原提交。请保留原操作号。";
            case LOCAL_RECEIPT_UNCONFIRMED -> "本机回执未确认。只能用原操作号重查。";
        };
    }

    private RemoteH2Exchange takePeer(String selected) {
        var editable = password.getText();
        char[] secret = new char[editable.length()];
        editable.getChars(0, editable.length(), secret, 0);
        editable.clear();
        try {
            peer = new RemoteH2Exchange(selected, account.getText().toString(), secret);
            return peer;
        } catch (IOException | IllegalArgumentException invalid) {
            status.setText("请检查 HTTPS 地址、账号和密码。");
            return null;
        } finally {
            Arrays.fill(secret, '\0');
        }
    }

    private int beginTask() {
        running = true;
        showControls();
        return ++epoch;
    }

    private boolean finishTask(int callback, RemoteH2Exchange exactPeer) {
        if (epoch != callback || !owns(exactPeer)) return false;
        pending = null;
        running = false;
        showControls();
        return true;
    }

    private boolean owns(RemoteH2Exchange candidate) {
        return live && candidate != null && peer == candidate && !isFinishing() && !isDestroyed();
    }

    private void releasePeer(RemoteH2Exchange exactPeer) {
        exactPeer.close();
        if (peer == exactPeer) peer = null;
    }

    private void showControls() {
        create.setEnabled(!running && !lockedNew);
        name.setEnabled(!running && !lockedNew);
        body.setEnabled(!running && !lockedNew);
        recover.setEnabled(!running);
        origin.setEnabled(!running);
        account.setEnabled(!running);
        password.setEnabled(!running);
        operation.setEnabled(!running);
    }

    @Override protected void onStart() {
        super.onStart();
        live = true;
    }

    @Override protected void onStop() {
        live = false;
        epoch++;
        if (peer != null) releasePeer(peer);
        if (pending != null) {
            pending.cancel(true);
            status.setText("界面已离开。若已发出提交，请用原操作号重查。");
        }
        pending = null;
        running = false;
        password.getText().clear();
        if (decision != null) decision.dismiss();
        showControls();
        super.onStop();
    }

    @Override protected void onDestroy() {
        worker.shutdownNow();
        super.onDestroy();
    }

    @Override protected void onSaveInstanceState(Bundle output) {
        super.onSaveInstanceState(output);
        String retained = operation.getText().toString();
        if (OPERATION.matcher(retained).matches()) output.putString("operation", retained);
        output.putBoolean("original_frozen", lockedNew);
    }

    private static String id() { return UUID.randomUUID().toString().replace("-", ""); }
}
