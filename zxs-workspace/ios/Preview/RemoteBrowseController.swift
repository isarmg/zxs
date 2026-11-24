import Foundation
import RemoteTransport
import UIKit

final class RemoteBrowseController: UITableViewController {
    private enum Page {
        case catalog(String?)
        case detail(RemoteRead.Catalog.Container, String?)
        case history(RemoteRead.Catalog.Container, RemoteRead.Detail.Head, String?, UInt64)

        func request() throws -> R3Request {
            switch self {
            case let .catalog(after): return try .catalog(after: after)
            case let .detail(container, after):
                return try .detail(container: container.containerId, sequence: container.sequence, after: after)
            case let .history(container, branch, cursor, _):
                return try .history(container: container.containerId, branch: branch.branch,
                                    head: branch.head, next: cursor)
            }
        }
    }

    private enum Row {
        case container(RemoteRead.Catalog.Container)
        case branch(RemoteRead.Catalog.Container, RemoteRead.Detail.Head)
        case node(RemoteRead.Catalog.Container, RemoteRead.Detail.Head, RemoteRead.History.Node)
    }

    private var exchange: MobileSaveExchange?
    private let store: RemoteSaveStore
    private let files: WholeFileStage
    private let onPrepared: (RemoteSave, Bool) -> Void
    private var trail: [Page] = [.catalog(nil)]
    private var rows: [Row] = []
    private var nextRemotePage: Page?
    private var job: Task<Void, Never>?
    private var token = UUID()
    private weak var consent: UIAlertController?
    private let banner = UILabel()

    init(exchange: MobileSaveExchange, store: RemoteSaveStore, files: WholeFileStage, onPrepared: @escaping (RemoteSave, Bool) -> Void) {
        self.exchange = exchange
        self.store = store
        self.files = files
        self.onPrepared = onPrepared
        super.init(style: .plain)
    }

    required init?(coder: NSCoder) { return nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "服务器文件"
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "row")
        banner.font = .preferredFont(forTextStyle: .footnote)
        banner.adjustsFontForContentSizeCategory = true
        banner.numberOfLines = 0
        banner.textAlignment = .center
        tableView.backgroundView = banner
        navigationItem.rightBarButtonItems = [
            UIBarButtonItem(title: "下一页", style: .plain, target: self, action: #selector(advance)),
            UIBarButtonItem(title: "上一级", style: .plain, target: self, action: #selector(ascend)),
        ]
        loadCurrent()
    }

    private func setNavigation() {
        navigationItem.rightBarButtonItems?.first?.isEnabled = nextRemotePage != nil && exchange != nil && job == nil
        navigationItem.rightBarButtonItems?.last?.isEnabled = trail.count > 1 && exchange != nil && job == nil
    }

    @objc private func advance() {
        guard let nextRemotePage, job == nil else { return }
        trail.append(nextRemotePage)
        loadCurrent()
    }

    @objc private func ascend() {
        guard trail.count > 1, job == nil else { return }
        trail.removeLast()
        loadCurrent()
    }

    private func loadCurrent() {
        guard let page = trail.last, let exchange, job == nil else { return }
        rows = []; nextRemotePage = nil; tableView.reloadData()
        banner.text = "读取服务器列表…"
        let selected = UUID()
        token = selected
        job = Task { [weak self] in
            do {
                let response = try await exchange.read(page.request())
                guard response.status == 200 else { throw RemoteRead.Failure.invalidPage }
                guard let self, self.token == selected, !Task.isCancelled else { return }
                switch page {
                case let .catalog(cursor):
                    let result = try RemoteRead.Catalog.select(response.bytes, after: cursor)
                    self.rows = result.items.map(Row.container)
                    self.nextRemotePage = result.nextAfter.map(Page.catalog)
                case let .detail(container, cursor):
                    let result = try RemoteRead.Detail.select(response.bytes, expected: container, after: cursor)
                    self.rows = result.heads.map { .branch(container, $0) }
                    self.nextRemotePage = result.nextAfter.map { .detail(container, $0) }
                case let .history(container, branch, cursor, upper):
                    let result = try RemoteRead.History.select(response.bytes, head: branch.head,
                                                               cursor: cursor, upper: upper)
                    self.rows = result.nodes.map { .node(container, branch, $0) }
                    if let next = result.next, let tail = result.nodes.last {
                        self.nextRemotePage = .history(container, branch, next, tail.sequence - 1)
                    }
                }
                self.banner.text = self.rows.isEmpty ? "本页没有项目。" : nil
                self.complete(selected)
            } catch {
                guard let self, self.token == selected, !Task.isCancelled else { return }
                self.banner.text = "无法读取原列表。数据可能已变，请返回重选。"
                self.complete(selected)
            }
        }
        setNavigation()
    }

    private func complete(_ selected: UUID) {
        guard token == selected else { return }
        job = nil
        tableView.reloadData()
        setNavigation()
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { rows.count }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "row", for: indexPath)
        var content = cell.defaultContentConfiguration()
        switch rows[indexPath.row] {
        case let .container(row): content.text = row.name; content.secondaryText = "版本 \(row.sequence)"
        case let .branch(_, branch): content.text = branch.branch; content.secondaryText = branch.head
        case let .node(_, _, node):
            content.text = "版本 \(node.sequence) · \(node.length) B"
            content.secondaryText = node.nodeId
        }
        content.textProperties.numberOfLines = 0
        cell.contentConfiguration = content
        cell.accessoryType = .disclosureIndicator
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        guard job == nil, exchange != nil, rows.indices.contains(indexPath.row) else { return }
        switch rows[indexPath.row] {
        case let .container(row): trail.append(.detail(row, nil)); loadCurrent()
        case let .branch(container, branch): trail.append(.history(container, branch, nil, container.sequence)); loadCurrent()
        case let .node(container, branch, node): offer(container, branch, node)
        }
    }

    private func offer(_ container: RemoteRead.Catalog.Container, _ branch: RemoteRead.Detail.Head,
                       _ node: RemoteRead.History.Node) {
        guard let exchange, presentedViewController == nil, consent == nil else { return }
        let eligible = node.nodeId == branch.head || (branch.branch == "main" && node.originalBranch == "main")
        let editable = eligible && node.length <= UInt64(LocalDraft.byteLimit)
        let alert = UIAlertController(title: eligible ? "选择版本操作" : "此节点无法产生新版本",
            message: eligible ? "\(container.name)\n\(branch.branch) · \(node.length) B\n\(node.nodeId)"
                : "非 main 分支只能更新当前节点。",
            preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "关闭", style: .cancel) { [weak self] _ in self?.consent = nil })
        if editable {
            alert.addAction(UIAlertAction(title: "读取并编辑", style: .default) { [weak self, weak alert] _ in
                guard let self, let alert, self.consent === alert,
                      self.view.window?.windowScene?.activationState == .foregroundActive else { return }
                self.consent = nil
                self.open(container, branch, node, over: exchange)
            })
        }
        if eligible {
            alert.addAction(UIAlertAction(title: "选择完整替换文件…", style: .default) { [weak self, weak alert] _ in
                guard let self, let alert, self.consent === alert,
                      self.navigationController?.topViewController === self,
                      self.view.window?.windowScene?.activationState == .foregroundActive else { return }
                self.consent = nil
                let target: WholeFileTarget.Selection = node.nodeId == branch.head
                    ? .append(branch: branch.branch, original: node)
                    : .forkMain(branch: RemoteDraftController.newID(), original: node)
                let plan = WholeFileTarget(destination: exchange.destination, container: container.containerId,
                    operation: RemoteDraftController.newID(), node: RemoteDraftController.newID(),
                    base: container.sequence, target: target)
                let editor = WholeFileImportController(plan: plan, files: self.files, drafts: self.store,
                    onPrepared: self.onPrepared)
                self.leaveForeground()
                self.navigationController?.pushViewController(editor, animated: true)
            })
        }
        consent = alert
        present(alert, animated: true)
    }

    private func open(_ container: RemoteRead.Catalog.Container, _ branch: RemoteRead.Detail.Head,
                      _ node: RemoteRead.History.Node, over exchange: MobileSaveExchange) {
        guard job == nil, let count = Int(exactly: node.length) else { return }
        let selected = UUID()
        token = selected
        navigationItem.prompt = "正在核验完整正文…"
        job = Task { [weak self] in
            do {
                let response = try await exchange.read(.textNode(container: container.containerId, node: node.nodeId,
                                                                 length: count, sha256: node.sha256))
                guard response.status == 200 else { throw RemoteRead.Failure.invalidPage }
                let selection = try RemoteRead.TextSelection(container: container, branch: branch, node: node,
                                                             original: response.bytes)
                guard let self, self.token == selected, !Task.isCancelled,
                      self.navigationController?.topViewController === self,
                      self.view.window?.windowScene?.activationState == .foregroundActive else { return }
                self.complete(selected)
                self.navigationItem.prompt = nil
                let editor = RemoteTextController(selection: selection, destination: exchange.destination,
                    store: self.store, onPrepared: { [onPrepared = self.onPrepared] saved in onPrepared(saved, true) })
                // Preserve only the verified body and destination identity;
                // the editor has no credentials and cannot send requests.
                self.leaveForeground()
                self.navigationController?.pushViewController(editor, animated: true)
            } catch {
                guard let self, self.token == selected, !Task.isCancelled else { return }
                self.complete(selected)
                self.navigationItem.prompt = "未取得完整、匹配的 UTF-8 正文。"
            }
        }
        setNavigation()
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isMovingFromParent || navigationController?.isBeingDismissed == true { leaveForeground() }
    }

    func leaveForeground() {
        consent?.dismiss(animated: false); consent = nil
        token = UUID()
        job?.cancel(); job = nil
        exchange = nil
        nextRemotePage = nil; rows = []
        if isViewLoaded {
            navigationItem.prompt = nil
            banner.text = "连接已关闭。请返回并重新选择服务器。"
            tableView.reloadData()
            setNavigation()
        }
    }
}

private final class RemoteTextController: UIViewController, UITextViewDelegate {
    private let selection: RemoteRead.TextSelection
    private let destination: String
    private let store: RemoteSaveStore
    private let onPrepared: (RemoteSave) -> Void
    private let operation = RemoteDraftController.newID(), node = RemoteDraftController.newID()
    private let branch = RemoteDraftController.newID()
    private let editor = UITextView(), status = UILabel()
    private var editorAccepted = false
    private var previousSwipe: Bool?

    init(selection: RemoteRead.TextSelection, destination: String, store: RemoteSaveStore,
         onPrepared: @escaping (RemoteSave) -> Void) {
        self.selection = selection
        self.destination = destination
        self.store = store
        self.onPrepared = onPrepared
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { return nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = selection.container.name
        view.backgroundColor = .systemBackground
        navigationItem.hidesBackButton = true
        navigationItem.leftBarButtonItem = UIBarButtonItem(title: "返回", style: .plain,
            target: self, action: #selector(close))
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: "准备提交", style: .done,
            target: self, action: #selector(prepare))
        navigationItem.rightBarButtonItem?.isEnabled = false
        editor.text = String(decoding: selection.original, as: UTF8.self)
        editorAccepted = selection.displayedExactly(editor.text)
        if !editorAccepted {
            // A renderer transformation (for example line-ending conversion)
            // cannot become the starting bytes of a later full replacement.
            editor.text = ""
            editor.isEditable = false
        }
        editor.font = .preferredFont(forTextStyle: .body)
        editor.adjustsFontForContentSizeCategory = true
        editor.delegate = self
        editor.autocorrectionType = .no
        editor.smartDashesType = .no
        editor.smartQuotesType = .no
        editor.smartInsertDeleteType = .no
        #if compiler(>=6.0)
        if #available(iOS 18.0, *) { editor.writingToolsBehavior = UIWritingToolsBehavior.none }
        #endif
        editor.accessibilityLabel = "远程文件的本地编辑"
        status.numberOfLines = 0
        status.font = .preferredFont(forTextStyle: .footnote)
        status.adjustsFontForContentSizeCategory = true
        status.text = !editorAccepted ? "不能原样编辑这份文本。远程原件保持不变。"
            : selection.node.nodeId == selection.branch.head ? "更新当前分支" : "从历史节点创建新分支"

        let layout = UIStackView(arrangedSubviews: [status, editor])
        layout.axis = .vertical
        layout.spacing = 8
        layout.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(layout)
        let safe = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            layout.topAnchor.constraint(equalTo: safe.topAnchor, constant: 8),
            layout.leadingAnchor.constraint(equalTo: safe.leadingAnchor, constant: 12),
            layout.trailingAnchor.constraint(equalTo: safe.trailingAnchor, constant: -12),
            layout.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor)
        ])
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        if let swipe = navigationController?.interactivePopGestureRecognizer {
            previousSwipe = swipe.isEnabled
            swipe.isEnabled = false
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        if let previousSwipe { navigationController?.interactivePopGestureRecognizer?.isEnabled = previousSwipe }
        previousSwipe = nil
    }

    func textViewDidChange(_ textView: UITextView) {
        navigationItem.rightBarButtonItem?.isEnabled = editorAccepted && !selection.displayedExactly(textView.text)
    }

    func textView(_ textView: UITextView, shouldChangeTextIn range: NSRange, replacementText text: String) -> Bool {
        guard editorAccepted, text.utf8.count <= LocalDraft.byteLimit, let old = textView.text,
              let bounds = Range(range, in: old) else { return false }
        let count = old.utf8.count - old[bounds].utf8.count + text.utf8.count
        if count > LocalDraft.byteLimit { status.text = "正文最多 256 KiB。" }
        return count <= LocalDraft.byteLimit
    }

    @objc private func close() {
        guard editorAccepted && (!selection.displayedExactly(editor.text) || editor.markedTextRange != nil) else {
            navigationController?.popViewController(animated: true)
            return
        }
        let alert = UIAlertController(title: "放弃未准备的更改？", message: nil, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "继续编辑", style: .cancel))
        alert.addAction(UIAlertAction(title: "放弃更改", style: .destructive) { [weak self] _ in
            self?.navigationController?.popViewController(animated: true)
        })
        present(alert, animated: true)
    }

    @objc private func prepare() {
        guard view.window?.windowScene?.activationState == .foregroundActive,
              editorAccepted, editor.markedTextRange == nil, !selection.displayedExactly(editor.text) else { return }
        do {
            let saved = try selection.freeze(text: editor.text, destination: destination, operationID: operation,
                                             newNode: node, newBranch: branch)
            try store.retain(saved)
            onPrepared(saved)
        } catch {
            status.text = "未能确认准备结果。保留输入；原操作号 \(operation)。"
        }
    }
}
