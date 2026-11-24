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
import android.window.OnBackInvokedCallback;
import android.window.OnBackInvokedDispatcher;

import java.io.IOException;
import java.math.BigInteger;
import java.nio.CharBuffer;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.function.Consumer;
import java.util.regex.Pattern;

/** Explicit first whole-text save and original-operation recovery. */
public final class RemoteSaveActivity extends Activity {
    private static final Pattern OPERATION = Pattern.compile("[0-9a-f]{32}");
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private EditText origin, account, password, name, body, operation;
    private Button create, recover, chooseLocal, browse, clearEdit, nextEdit;
    private TextView status;
    private Future<?> pending;
    private AlertDialog decision;
    private RemoteH2Exchange peer;
    private RemoteContainerRead.Selection selectedFile;
    private final OnBackInvokedCallback leave = this::requestExit;
    private boolean running, live, lockedNew, bodyRetained, originalConfirmed;
    private String lockedOperation;
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
        chooseLocal = button(form, "从本机选择原操作…", () -> listLocal(null));
        recover = button(form, "查看原提交…", this::loadOriginal);
        browse = button(form, "浏览已有远程文件…", this::beginBrowse);
        clearEdit = button(form, "关闭本次文件编辑…", this::clearEdited);
        nextEdit = button(form, "开始另一个操作", this::resetAfterConfirmed);
        name = field(form, "新文件名", InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS, 255, true);
        body = field(form, "全文", InputType.TYPE_TEXT_FLAG_MULTI_LINE, PlainText.LIMIT, false);
        body.setMinLines(6);
        create = button(form, "提交新文件…", this::firstDecision);
        if (saved != null) {
            operation.setText(saved.getString("operation", ""));
            lockedNew = saved.getBoolean("original_frozen");
            lockedOperation = saved.getString("locked_operation");
        }
        Object retained = getLastNonConfigurationInstance();
        if (retained instanceof InMemoryDraft draft) {
            selectedFile = draft.selected();
            bodyRetained = draft.bodyRetained();
            originalConfirmed = draft.originalConfirmed();
            origin.setText(draft.origin());
            account.setText(draft.account());
            name.setText(draft.name());
            body.setText(draft.text());
        }
        setContentView(scroll);
        getOnBackInvokedDispatcher().registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, leave);
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

    private record InMemoryDraft(RemoteContainerRead.Selection selected, String origin, String account,
                                 String name, String text, boolean bodyRetained, boolean originalConfirmed) {}

    @Override public Object onRetainNonConfigurationInstance() {
        return new InMemoryDraft(selectedFile, origin.getText().toString(), account.getText().toString(),
                name.getText().toString(), body.getText().toString(), bodyRetained, originalConfirmed);
    }

    private void firstDecision() {
        if (!live || running || decision != null || lockedNew) return;
        byte[] raw;
        RemoteSaveContract.Save save;
        try {
            String text = body.getText().toString();
            if (PlainText.scanPages(text, () -> {}, null) < 0) throw new IOException("unsupported text");
            var encoded = StandardCharsets.UTF_8.newEncoder().encode(CharBuffer.wrap(text));
            if (encoded.remaining() > PlainText.LIMIT) throw new IOException("too long");
            raw = new byte[encoded.remaining()];
            encoded.get(raw);
            if (selectedFile == null) {
                var target = new RemoteSaveContract.FirstMain(name.getText().toString());
                save = new RemoteSaveContract.Save(id(), id(), id(), BigInteger.ZERO, target, raw.length,
                        ArchiveIndex.hex(ArchiveIndex.sha256().digest(raw)));
            } else {
                save = selectedFile.changedMain(id(), id(), selectedFile.historical() ? id() : null, raw).save();
            }
        } catch (IllegalArgumentException | IOException invalid) {
            status.setText(selectedFile == null ? "请检查文件名、文本格式和长度。"
                    : "需要修改原文件；请检查文本格式和长度。");
            return;
        }

        String selected = origin.getText().toString();
        var exactPeer = takePeer(selected);
        if (exactPeer == null) {
            Arrays.fill(raw, (byte) 0);
            return;
        }
        if (selectedFile != null && !selectedFile.authority().equals(exactPeer.authority())) {
            releasePeer(exactPeer);
            Arrays.fill(raw, (byte) 0);
            status.setText("请使用选定文件的原地址和账号。未提交。");
            return;
        }
        var plan = new RemoteSaveJournal.Plan(exactPeer.authority(), save);
        decision = new AlertDialog.Builder(this).setTitle("确认提交完整文件")
                .setMessage(summary(selected, plan))
                .setNegativeButton("取消", null)
                .setPositiveButton("提交一次", (dialog, which) -> {
                    if (!owns(exactPeer) || lockedNew) return;
                    lockedNew = true;
                    lockedOperation = save.operation();
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

    private void beginBrowse() {
        if (!live || running || decision != null || lockedNew || selectedFile != null) return;
        if (body.length() != 0 || name.length() != 0) {
            status.setText("请先处理或清空新建文件的正文和名称，再打开已有文件。");
            return;
        }
        var exactPeer = takePeer(origin.getText().toString());
        if (exactPeer != null) browsePage(exactPeer, null);
    }

    private void browsePage(RemoteH2Exchange exactPeer, String after) {
        readRemote(exactPeer, () -> RemoteContainerRead.catalog(exactPeer, exactPeer.authority(), after),
                page -> showRemotePage(exactPeer, page));
    }

    private void showRemotePage(RemoteH2Exchange exactPeer, RemoteContainerRead.Catalog page) {
        if (page.items().isEmpty()) {
            status.setText("当前没有可选的远程文件。");
            releasePeer(exactPeer);
            return;
        }
        var rows = page.items();
        String[] names = new String[rows.size()];
        for (int index = 0; index < rows.size(); index++) {
            var item = rows.get(index);
            names[index] = item.name() + "\n" + item.container() + " · " + item.sequence();
        }
        var builder = new AlertDialog.Builder(this).setTitle("选择已有远程文件")
                .setItems(names, (dialog, index) -> {
                    if (dialog != decision || !owns(exactPeer)) return;
                    readRemote(exactPeer, () ->
                            RemoteContainerRead.branches(exactPeer, exactPeer.authority(), rows.get(index)),
                            branches -> chooseBranch(exactPeer, branches));
                }).setNegativeButton("取消", null);
        if (page.next() != null) {
            builder.setNeutralButton("下一页", (dialog, which) -> {
                if (dialog == decision) browsePage(exactPeer, page.next());
            });
        }
        showPeerDialog(builder.create(), exactPeer);
    }

    private void chooseBranch(RemoteH2Exchange exactPeer, RemoteContainerRead.Branches original) {
        var heads = original.heads();
        if (heads.size() == 1) {
            openBranch(exactPeer, original, heads.get(0));
            return;
        }
        String[] labels = new String[heads.size()];
        for (int index = 0; index < heads.size(); index++) {
            var head = heads.get(index);
            labels[index] = head.branch() + "\n" + head.node();
        }
        var chooser = new AlertDialog.Builder(this).setTitle("选择原分支")
                .setItems(labels, (dialog, index) -> {
                    if (dialog == decision) openBranch(exactPeer, original, heads.get(index));
                }).setNegativeButton("取消", null).create();
        showPeerDialog(chooser, exactPeer);
    }

    private void openBranch(RemoteH2Exchange exactPeer, RemoteContainerRead.Branches original,
                            RemoteContainerRead.Head choice) {
        readRemote(exactPeer, () -> RemoteContainerRead.selectBranch(exactPeer, original, choice),
                selected -> confirmDownload(exactPeer, selected));
    }

    private void confirmDownload(RemoteH2Exchange exactPeer, RemoteContainerRead.Selection choice) {
        var rev = choice.revision();
        boolean supported = rev.length().compareTo(BigInteger.valueOf(PlainText.LIMIT)) <= 0;
        String rule = choice.historical() ? "提交修改会建立新分支，不改 main。"
                : "提交修改会追加到已选分支头。";
        String details = choice.item().name() + "\n" + choice.branch() + " · " + rev.node() + "\n" + rev.length() + " 字节\n"
                + (supported ? rule : "该文件超过当前全文编辑上限。");
        var builder = new AlertDialog.Builder(this).setTitle("原节点")
                .setMessage(details).setNegativeButton("取消", null);
        if (!choice.historical() && "main".equals(choice.branch())) {
            builder.setNeutralButton("查看 main 历史", (dialog, which) -> {
                if (dialog == decision) browseHistory(exactPeer, choice, null);
            });
        }
        if (supported) {
            builder.setPositiveButton("读取并编辑", (dialog, which) -> {
                if (dialog != decision || !owns(exactPeer)) return;
                readRemote(exactPeer, () -> {
                    byte[] raw = choice.readExact(exactPeer);
                    try {
                        String content = StandardCharsets.UTF_8.newDecoder()
                                .decode(java.nio.ByteBuffer.wrap(raw)).toString();
                        if (PlainText.scanPages(content, () -> {}, null) < 0)
                            throw new IOException("not editable text");
                        return content;
                    } finally {
                        Arrays.fill(raw, (byte) 0);
                    }
                }, content -> {
                    selectedFile = choice;
                    name.setText(choice.item().name());
                    body.setText(content);
                    status.setText((choice.historical() ? "已打开历史节点。提交修改会建立新分支。"
                            : "已打开原节点。修改后可提交到 " + choice.branch() + "。") + "\n" + rev.node());
                    showControls();
                    releasePeer(exactPeer);
                });
            });
        }
        showPeerDialog(builder.create(), exactPeer);
    }

    private void browseHistory(RemoteH2Exchange exactPeer, RemoteContainerRead.Selection anchor,
                               RemoteContainerRead.Cursor after) {
        readRemote(exactPeer, () -> RemoteContainerRead.mainHistory(exactPeer, anchor, after), page -> {
            var nodes = page.nodes();
            String[] labels = new String[nodes.size()];
            for (int index = 0; index < nodes.size(); index++) {
                var node = nodes.get(index);
                labels[index] = "版本 " + node.sequence() + " · " + node.length() + " 字节\n" + node.node();
            }
            var builder = new AlertDialog.Builder(this).setTitle("main 原历史")
                    .setItems(labels, (dialog, index) -> {
                        if (dialog != decision || !owns(exactPeer)) return;
                        confirmDownload(exactPeer, page.select(index));
                    }).setNegativeButton("取消", null);
            if (page.next() != null) {
                builder.setNeutralButton("下一页", (dialog, which) -> {
                    if (dialog == decision) browseHistory(exactPeer, anchor, page.next());
                });
            }
            showPeerDialog(builder.create(), exactPeer);
        });
    }

    private void showPeerDialog(AlertDialog chooser, RemoteH2Exchange exactPeer) {
        decision = chooser;
        chooser.setOnDismissListener(dialog -> {
            if (decision == chooser) {
                decision = null;
                if (!running) releasePeer(exactPeer);
            }
        });
        chooser.show();
    }

    @FunctionalInterface
    private interface Reading<T> { T read() throws IOException; }

    private <T> void readRemote(RemoteH2Exchange exactPeer, Reading<T> reading, Consumer<T> done) {
        if (!owns(exactPeer) || running) return;
        int current = beginTask();
        pending = worker.submit(() -> {
            T value;
            try { value = reading.read(); }
            catch (IOException | RuntimeException invalid) { value = null; }
            T delivered = value;
            runOnUiThread(() -> {
                if (!finishTask(current, exactPeer)) return;
                if (delivered == null) {
                    status.setText("无法读取所选原文件。没有发送正文。");
                    releasePeer(exactPeer);
                } else {
                    done.accept(delivered);
                }
            });
        });
    }

    private void clearEdited() {
        if (!live || running || decision != null || lockedNew || selectedFile == null) return;
        if (hasUnsavedEditor()) confirmDiscard("关闭文件编辑？", this::clearEditor);
        else clearEditor();
    }

    private void requestExit() {
        if (!live || decision != null || isFinishing()) return;
        if (hasUnsavedEditor()) confirmDiscard("离开文件编辑？", this::finish);
        else finish();
    }

    private void confirmDiscard(String title, Runnable confirmAction) {
        var confirm = new AlertDialog.Builder(this).setTitle(title)
                .setMessage("尚未留在本机的编辑不会保存。")
                .setNegativeButton("取消", null)
                .setPositiveButton("离开", (dialog, which) -> {
                    if (live && dialog == decision) confirmAction.run();
                }).create();
        decision = confirm;
        confirm.setOnDismissListener(dialog -> { if (decision == confirm) decision = null; });
        confirm.show();
    }

    private boolean hasUnsavedEditor() {
        if (bodyRetained) return false;
        if (selectedFile == null) return body.length() != 0 || name.length() != 0;
        String text = body.getText().toString();
        try {
            var encoded = StandardCharsets.UTF_8.newEncoder().encode(CharBuffer.wrap(text));
            return selectedFile.differs(encoded);
        } catch (IOException invalid) {
            return true;
        }
    }

    private void clearEditor() {
        selectedFile = null;
        name.setText("");
        body.setText("");
        status.setText("已关闭。未提交。");
        showControls();
    }

    private void resetAfterConfirmed() {
        if (!live || running || decision != null || !originalConfirmed) return;
        // The synchronized durable original receipt remains in the journal and
        // the operation field. Starting anew cannot reuse the old baseline.
        selectedFile = null;
        lockedNew = false;
        lockedOperation = null;
        bodyRetained = false;
        originalConfirmed = false;
        name.setText("");
        body.setText("");
        showControls();
    }

    private void listLocal(String after) {
        if (!live || running || decision != null) return;
        RemoteSaveJournal.Authority exact;
        try {
            var selected = RemoteH2Exchange.Origin.parse(origin.getText().toString());
            exact = new RemoteSaveJournal.Authority(selected.identity(), account.getText().toString());
        } catch (IllegalArgumentException invalid) {
            status.setText("请输入原服务器的 HTTPS 地址和账号。");
            return;
        }
        Context context = getApplicationContext();
        int current = beginTask();
        pending = worker.submit(() -> {
            RemoteSaveJournal.Page page;
            try {
                page = RemoteSaveJournal.plans(context, exact, after);
            } catch (IOException | RuntimeException unavailable) {
                page = null;
            }
            var delivered = page;
            runOnUiThread(() -> {
                if (!live || epoch != current || isFinishing() || isDestroyed()) return;
                pending = null;
                running = false;
                showControls();
                if (delivered == null) {
                    status.setText("本机原操作列表不可用。已知原操作号时仍可按原号核对。");
                } else if (delivered.rows().isEmpty()) {
                    status.setText("此服务器和账号没有可选的本机原操作。");
                } else {
                    showLocalPage(delivered);
                }
            });
        });
    }

    private void showLocalPage(RemoteSaveJournal.Page page) {
        var rows = page.rows();
        String[] choices = new String[rows.size()];
        for (int index = 0; index < rows.size(); index++) {
            var original = rows.get(index).save();
            String label = original.target() instanceof RemoteSaveContract.FirstMain first
                    ? first.name() : original.container() + " / " + original.target().branch();
            choices[index] = label + "\n" + original.operation();
        }
        var builder = new AlertDialog.Builder(this).setTitle("本机原操作")
                .setItems(choices, (dialog, index) -> {
                    if (!live || running || dialog != decision) return;
                    operation.setText(rows.get(index).save().operation());
                    status.setText("已选原操作。输入密码后可核对并选择查询或提交。");
                })
                .setNegativeButton("取消", null);
        if (page.next() != null) {
            builder.setNeutralButton("下一页", (dialog, which) -> {
                if (dialog != decision) return;
                decision = null;
                listLocal(page.next());
            });
        }
        var chooser = builder.create();
        decision = chooser;
        chooser.setOnDismissListener(dialog -> {
            if (decision == chooser) decision = null;
        });
        chooser.show();
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
            boolean preparedHere = false;
            try {
                if (firstBody != null) {
                    RemoteSaveJournal.prepare(context, plan.authority(), plan.save(), firstBody);
                    preparedHere = true;
                }
                result = RemoteSaveSubmit.execute(context, plan, mode, exactPeer);
            } catch (IOException | RuntimeException refused) {
                result = new RemoteSaveSubmit.Result(RemoteSaveSubmit.Status.LOCAL_UNAVAILABLE, null);
            } finally {
                if (firstBody != null) Arrays.fill(firstBody, (byte) 0);
                exactPeer.close();
            }
            var delivered = result;
            boolean leftOnDisk = preparedHere;
            runOnUiThread(() -> {
                if (!finishTask(current, exactPeer)) return;
                releasePeer(exactPeer);
                if (plan.save().operation().equals(lockedOperation)) {
                    bodyRetained |= leftOnDisk || delivered.status() == RemoteSaveSubmit.Status.CONFIRMED;
                    originalConfirmed |= delivered.status() == RemoteSaveSubmit.Status.CONFIRMED;
                    showControls();
                }
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
        create.setText(selectedFile == null ? "提交新文件…" : selectedFile.historical()
                ? "提交完整修改到新分支…" : "提交完整修改…");
        browse.setEnabled(!running && !lockedNew && selectedFile == null);
        clearEdit.setVisibility(selectedFile == null ? View.GONE : View.VISIBLE);
        clearEdit.setEnabled(!running && !lockedNew);
        nextEdit.setVisibility(originalConfirmed ? View.VISIBLE : View.GONE);
        nextEdit.setEnabled(!running && originalConfirmed);
        name.setEnabled(!running && !lockedNew && selectedFile == null);
        body.setEnabled(!running && !lockedNew);
        chooseLocal.setEnabled(!running);
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
        getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(leave);
        worker.shutdownNow();
        super.onDestroy();
    }

    @Override protected void onSaveInstanceState(Bundle output) {
        super.onSaveInstanceState(output);
        String retained = operation.getText().toString();
        if (OPERATION.matcher(retained).matches()) output.putString("operation", retained);
        output.putBoolean("original_frozen", lockedNew);
        output.putString("locked_operation", lockedOperation);
    }

    private static String id() { return UUID.randomUUID().toString().replace("-", ""); }
}
