import UIKit
import UniformTypeIdentifiers

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {}

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        let w = UIWindow(windowScene: scene)
        w.rootViewController = PreviewController()
        self.window = w; w.makeKeyAndVisible()
    }
    func sceneDidEnterBackground(_ scene: UIScene) {
        (window?.rootViewController as? PreviewController)?.clearSelection()
    }
    func sceneDidDisconnect(_ scene: UIScene) {
        (window?.rootViewController as? PreviewController)?.clearSelection()
    }
}

private final class NameSearch {
    let operation = BlockOperation()
    let root: Member, folder: Member, page: Int
    weak var archive: HeldArchive?
    weak var sheet: UINavigationController?
    var consumed = false

    init(_ view: Report, folder: Member, page: Int) {
        root = view.directory.root; archive = view.archive; self.folder = folder; self.page = page
    }
}

// One immutable result list. The sheet never replaces its rows or interprets a
// path. It holds directory declarations, not the document or its access grant.
private final class NameResults: UITableViewController {
    let matches: [DirectoryIndex.NameMatch]
    var finish: ((DirectoryIndex.NameMatch?) -> Void)?

    init(_ matches: [DirectoryIndex.NameMatch]) {
        self.matches = matches
        super.init(style: .plain)
        title = "名称匹配 \(matches.count) 项"
    }
    required init?(coder: NSCoder) { return nil }

    override func viewDidLoad() {
        super.viewDidLoad()
        navigationItem.leftBarButtonItem = UIBarButtonItem(title: "返回目录", style: .plain,
            target: self, action: #selector(back))
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "match")
        tableView.rowHeight = UITableView.automaticDimension
    }
    @objc private func back() { finish?(nil) }
    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        matches.count
    }
    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let hit = matches[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "match", for: indexPath)
        var content = cell.defaultContentConfiguration()
        content.text = hit.path
        content.textProperties.numberOfLines = 0
        content.textProperties.lineBreakMode = .byCharWrapping
        content.secondaryText = hit.member.children == nil ? "\(hit.member.size) B（声明）" : "目录"
        cell.contentConfiguration = content
        return cell
    }
    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        guard tableView === self.tableView, indexPath.section == 0, matches.indices.contains(indexPath.row) else { return }
        finish?(matches[indexPath.row])
    }
}

final class PreviewController: UIViewController, UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate,
                               UITableViewDataSource, UITableViewDelegate {
    private let queue: OperationQueue = {
        let q = OperationQueue(); q.maxConcurrentOperationCount = 1; q.qualityOfService = .userInitiated; return q
    }()
    private let status = UILabel(), location = UILabel(), tableHost = UIStackView()
    private var table = UITableView()
    private let cancelButton = UIButton(type: .system), up = UIButton(type: .system)
    private let find = UIButton(type: .system), form = UIStackView(), query = UITextField()
    private var search: NameSearch?
    private let previous = UIButton(type: .system), next = UIButton(type: .system)
    private let closeText = UIButton(type: .system), literal = UITextView()
    private weak var awaiting: UIDocumentPickerViewController?
    private var active: DocumentRead?
    private var selection: DocumentPresentation?, report: Report?
    private weak var decision: UIAlertController?
    private var trail: [Member] = [], children: [Member] = [], page = 0
    private let pageSize = 100

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        let notice = UILabel()
        for label in [notice, status, location] { label.numberOfLines = 0; label.adjustsFontForContentSizeCategory = true }
        notice.font = .preferredFont(forTextStyle: .headline)
        notice.text = "ZIP 目录预览\n目录只显示声明；可核验单项 CRC-32，或校验并查看不超过 32 KiB 的 UTF-8 纯文本。不认证快照；外部提供方可能自行下载。"
        status.font = .preferredFont(forTextStyle: .body); status.numberOfLines = 5
        location.font = .preferredFont(forTextStyle: .footnote)
        location.numberOfLines = 2
        func wire(_ button: UIButton, _ title: String, _ action: Selector) {
            button.setTitle(title, for: .normal); button.addTarget(self, action: action, for: .touchUpInside)
        }
        let pick = UIButton(type: .system)
        wire(pick, "选择 .zxs 或 Store ZIP", #selector(choose))
        wire(cancelButton, "取消并关闭文件", #selector(cancelRead))
        wire(find, "按名称查找…", #selector(offerFind))
        let submit = UIButton(type: .system), fold = UIButton(type: .system)
        wire(submit, "查找", #selector(beginFind))
        wire(fold, "收起", #selector(clearDraft))
        query.placeholder = "名称片段，区分大小写"
        query.accessibilityLabel = "归档成员名称片段"
        query.borderStyle = .roundedRect
        query.autocapitalizationType = .none; query.autocorrectionType = .no; query.spellCheckingType = .no
        query.smartQuotesType = .no; query.smartDashesType = .no; query.smartInsertDeleteType = .no
        #if compiler(>=6.0)
        if #available(iOS 18.0, *) { query.writingToolsBehavior = UIWritingToolsBehavior.none }
        #endif
        form.axis = .horizontal; form.spacing = 6
        for child in [query, submit, fold] { form.addArrangedSubview(child) }
        wire(up, "上一级", #selector(goUp))
        wire(previous, "上一页", #selector(prevPage))
        wire(next, "下一页", #selector(nextPage))
        let nav = UIStackView(arrangedSubviews: [up, previous, next]); nav.distribution = .fillEqually
        wire(closeText, "关闭文本，返回目录", #selector(closePreview))
        // The text view occupies the available viewport, not a view as tall as every line.
        literal.isScrollEnabled = true; literal.alwaysBounceHorizontal = false
        literal.isEditable = false; literal.isSelectable = false; literal.dataDetectorTypes = []
        literal.allowsEditingTextAttributes = false; literal.textDragInteraction?.isEnabled = false
        literal.autocorrectionType = .no; literal.spellCheckingType = .no
        #if compiler(>=6.0)
        if #available(iOS 18.0, *) { literal.writingToolsBehavior = UIWritingToolsBehavior.none }
        #endif
        literal.textContainer.maximumNumberOfLines = 0; literal.textContainer.lineBreakMode = .byCharWrapping
        literal.font = .preferredFont(forTextStyle: .body); literal.adjustsFontForContentSizeCategory = true
        literal.isAccessibilityElement = true
        tableHost.axis = .vertical
        let stack = UIStackView(arrangedSubviews: [notice, pick, find, form, cancelButton, status, location, nav,
                                                   closeText, literal, tableHost])
        stack.axis = .vertical; stack.spacing = 5; stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([stack.topAnchor.constraint(equalTo: guide.topAnchor, constant: 8),
            stack.bottomAnchor.constraint(equalTo: guide.bottomAnchor), stack.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 12),
            stack.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -12)])
        reset("请选择一份文件。")
    }

    private func retireSearch() {
        let oldSearch = search; search = nil
        oldSearch?.operation.cancel()
        if let sheet = oldSearch?.sheet, presentedViewController === sheet {
            sheet.viewIfLoaded?.isHidden = true
            sheet.dismiss(animated: false)
        }
    }

    func clearSelection() {
        awaiting = nil; active?.cancel(); active = nil
        retireSearch()
        clearDraft()
        queue.cancelAllOperations()
        decision?.dismiss(animated: false); decision = nil
        let previous = selection; selection = nil; previous?.retire()
        report = nil
        if isViewLoaded { reset("没有保留上次选择。") }
    }
    @objc private func cancelRead() {
        if search != nil {
            retireSearch()
            status.text = "已取消查找，保留原目录与页。"
            render()
        } else { clearSelection(); reset("已取消本次读取。") }
    }

    @objc private func choose() {
        guard presentedViewController == nil else { return }
        clearSelection(); reset("等待系统文档选择器…")
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.data], asCopy: false)
        picker.allowsMultipleSelection = false; picker.shouldShowFileExtensions = true
        picker.delegate = self; awaiting = picker
        present(picker, animated: true)
        picker.presentationController?.delegate = self
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        guard awaiting === controller else { return }
        awaiting = nil; reset("没有选择文件。")
    }
    func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
        if let p = awaiting, presentationController.presentedViewController === p { documentPickerWasCancelled(p) }
    }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard awaiting === controller else { return }
        awaiting = nil
        guard urls.count == 1, let url = urls.first else { reset("仅支持选择一份文件。"); return }
        let job = DocumentRead(), scope: DocumentPresentation
        do {
            scope = try job.present(url) { [weak self] lost in
                DispatchQueue.main.async { [weak self] in
                    guard let self = self, self.selection === lost else { return }
                    self.clearSelection(); self.reset("外部文档已变化或需让出访问。请重新选择。")
                }
            }
        } catch { reset("未获得这份文件的临时读取权。"); return }
        selection = scope; active = job; reset("正在读取目录声明…")
        queue.addOperation { [weak self] in
            let outcome = Result { try job.run(scope) }
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.active === job, self.selection === scope else { return }
                self.active = nil
                guard (try? scope.check()) != nil else { self.clearSelection(); return }
                switch outcome {
                case .success(let report):
                    self.report = report
                    self.reset(report.name + "（本次目录记录）")
                    self.trail = [report.directory.root]; self.enter()
                case .failure:
                    self.clearSelection()
                    self.reset("无法预览。文件不可用、授权不足、读取时变化或 ZIP 目录超出支持范围。")
                }
            }
        }
    }
    private var mayNavigate: Bool {
        active == nil && search == nil && literal.isHidden && presentedViewController == nil
    }

    @objc private func offerFind() {
        guard mayNavigate, let report = report, selection === report.archive.scope,
              (try? report.archive.scope.check()) != nil else { return }
        form.isHidden = false; query.becomeFirstResponder()
    }

    @objc private func clearDraft() {
        query.resignFirstResponder()
        query.text = nil
        form.isHidden = true
    }

    @objc private func beginFind() {
        guard mayNavigate, !form.isHidden, let report = report, let folder = trail.last,
              selection === report.archive.scope, (try? report.archive.scope.check()) != nil else { return }
        guard let term = query.text, !term.isEmpty, term.utf16.count <= DirectoryIndex.maxSearchUnits,
              query.markedTextRange == nil else {
            status.text = "请输入名称片段，完成输入后再查找（最多 128 个 UTF-16 单元）。"
            return
        }
        clearDraft()
        let request = NameSearch(report, folder: folder, page: page)
        let directory = report.directory  // Worker retains this only, never Report, FD or document scope.
        search = request
        status.text = "正在本次目录记录中查找名称…"
        render()
        request.operation.addExecutionBlock { [weak self, weak request] in
            guard let operation = request?.operation else { return }
            let outcome = Result { try directory.findNames(term) {
                if operation.isCancelled { throw PreviewError.cancelled }
            }}
            DispatchQueue.main.async { [weak self, weak request] in
                guard let self = self, let request = request, self.search === request else { return }
                self.showMatches(outcome, request: request)
            }
        }
        queue.addOperation(request.operation)
    }

    private func originalArchive(for request: NameSearch) -> Report? {
        guard let view = report, view.archive === request.archive, view.directory.root === request.root,
              selection === view.archive.scope, (try? view.archive.scope.check()) != nil else { return nil }
        return view
    }

    private func showMatches(_ outcome: Result<[DirectoryIndex.NameMatch], Error>, request: NameSearch) {
        guard search === request, !request.consumed, request.sheet == nil else { return }
        guard originalArchive(for: request) != nil else { clearSelection(); return }
        guard trail.last === request.folder, page == request.page,
              viewIfLoaded?.window != nil, presentedViewController == nil else {
            search = nil; status.text = "未应用已离开的目录查找。"; render(); return
        }
        guard case .success(let matches) = outcome, !matches.isEmpty else {
            search = nil
            if case .success = outcome { status.text = "没有匹配的文件或目录名称。" }
            else { status.text = "未完成名称查找，仍可浏览原目录。" }
            render()
            return
        }
        let results = NameResults(matches)
        let sheet = UINavigationController(rootViewController: results)
        sheet.modalPresentationStyle = .pageSheet; sheet.isModalInPresentation = true
        results.finish = { [weak self, weak request] match in
            if let request = request { self?.closeMatches(request, match: match) }
        }
        request.sheet = sheet
        present(sheet, animated: true)
    }

    private func closeMatches(_ request: NameSearch, match: DirectoryIndex.NameMatch?) {
        guard search === request, !request.consumed, let sheet = request.sheet,
              presentedViewController === sheet, !sheet.isBeingPresented, !sheet.isBeingDismissed else { return }
        request.consumed = true
        sheet.dismiss(animated: true) { [weak self, weak request] in
            guard let self = self, let request = request, self.search === request else { return }
            self.search = nil
            guard let origin = self.originalArchive(for: request) else { self.clearSelection(); return }
            guard self.trail.last === request.folder, self.page == request.page else {
                self.status.text = "原目录页已变，未应用查找结果。"; self.render(); return
            }
            if let match = match, let rows = origin.directory.originalParentRows(for: match) {
                guard self.originalArchive(for: request) != nil else { self.clearSelection(); return }
                self.trail = match.parents
                self.children = rows
                self.page = match.ordinal / self.pageSize
                self.status.text = "已定位到所选条目所在页；未打开成员。"
                self.render(focus: match.ordinal % self.pageSize)
            } else {
                self.status.text = match == nil ? origin.name + "（本次目录记录）" : "不能核对原条目，仍在原目录。"
                self.render()
            }
        }
    }

    private func offer(_ file: Member) {
        guard mayNavigate, let view = report, selection === view.archive.scope,
              (try? view.archive.scope.check()) != nil else { return }
        clearDraft()
        let path = "/" + (trail.dropFirst().map(\.name) + [file.name]).joined(separator: "/")
        let small = file.size <= PlainText.byteLimit
        let choice = small ? "只有 CRC 相符且可显示的 UTF-8 才提供文本。" : "超过 32 KiB，仅可核验 CRC。"
        let alert = UIAlertController(title: "查看这个归档成员",
            message: "\(view.name)\n\(path)\n将读取全部 \(file.size) 字节。\(choice)不保存或上传。外部提供方可能自行下载。",
            preferredStyle: .alert)
        func action(_ title: String, text: Bool) -> UIAlertAction {
            UIAlertAction(title: title, style: .default) { [weak self] _ in
                guard let self = self else { return }; self.decision = nil
                guard self.active == nil, self.showing(view, file: file), (try? view.archive.scope.check()) != nil else { return }
                self.verify(view, file: file, path: path, previewText: text)
            }
        }
        alert.addAction(UIAlertAction(title: "返回目录", style: .cancel) { [weak self] _ in self?.decision = nil })
        alert.addAction(action("只核验 CRC", text: false))
        if small { alert.addAction(action("校验并看文本", text: true)) }
        decision = alert; present(alert, animated: true)
    }
    private func showing(_ record: Report, file: Member) -> Bool {
        guard search == nil, selection === record.archive.scope, report?.archive === record.archive else { return false }
        let first = page * pageSize
        return first < children.count && children[first..<min(first + pageSize, children.count)].contains { $0 === file }
    }
    private func verify(_ view: Report, file: Member, path: String, previewText: Bool) {
        let job = DocumentRead(); active = job
        status.text = "正在读取 \(path)（\(file.size) B）…"; render()
        queue.addOperation { [weak self] in
            let outcome = Result { try job.verify(view, file: file, previewText: previewText) }
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.active === job, self.selection === view.archive.scope else { return }
                self.active = nil
                guard self.showing(view, file: file), (try? view.archive.scope.check()) != nil else { self.clearSelection(); return }
                switch outcome {
                case .success(let result):
                    let verdict = result.matches ? "该项 CRC-32 与 ZIP 声明相符。" : "该项 CRC-32 与 ZIP 声明不符。"
                    let values = String(format: "声明 %08X，实读 %08X。", result.declared, result.actual)
                    let plainNotice = previewText && result.matches && result.text == nil
                        ? "\n不是此预览支持的 UTF-8 纯文本，不显示正文。" : ""
                    self.status.text = "\(verdict)\n\(values) 不证明整包或其他项。\(plainNotice)\n\(path)（\(file.size) B）"
                    self.render()
                    if previewText && result.matches, let text = result.text { self.showText(text) }
                case .failure:
                    self.clearSelection()
                    self.reset("没有完整核验结果：布局不符、原文件变化或读取不可用。请重新选择。")
                }
            }
        }
    }
    private func showText(_ text: String) {
        location.text = text.isEmpty ? "空文本（0 字节），CRC-32 相符。" : "全文预览：该项 CRC-32 相符。"
        status.numberOfLines = 3; tableHost.isHidden = true
        literal.text = text // never attributed text, a web view, data detector or an external-open action
        literal.isHidden = false; closeText.isHidden = false; literal.setContentOffset(.zero, animated: false)
        find.isEnabled = false; up.isEnabled = false; previous.isEnabled = false; next.isEnabled = false
    }
    private func clearText() {
        literal.text = nil; literal.isHidden = true; closeText.isHidden = true
        tableHost.isHidden = false; status.numberOfLines = 5
    }
    @objc private func closePreview() {
        clearText()
        if let report = report, selection === report.archive.scope, (try? report.archive.scope.check()) != nil {
            render()
        } else { clearSelection() }
    }
    private func reset(_ message: String) {
        clearDraft(); clearText()
        trail = []; children = []; page = 0
        status.text = message; location.text = nil; cancelButton.isHidden = active == nil
        cancelButton.setTitle("取消并关闭文件", for: .normal)
        find.isEnabled = false; up.isEnabled = false; previous.isEnabled = false; next.isEnabled = false
        replacePage(canSelect: false, focus: 0)
    }
    private func enter() {
        page = 0
        children = trail.last.map { DirectoryIndex.orderedChildren(of: $0) } ?? []
        render()
    }
    private func render(focus: Int = 0) {
        let pages = max(1, (children.count + pageSize - 1) / pageSize)
        let pageText = children.isEmpty ? "空目录" : "\(page + 1)/\(pages) 页"
        location.text = pageText + "   /" + trail.dropFirst().map(\.name).joined(separator: "/")
        let idle = active == nil && search == nil
        let finding = search.map { $0.sheet == nil && !$0.consumed } ?? false
        find.isEnabled = idle && report != nil && literal.isHidden
        cancelButton.setTitle(finding ? "取消查找" : "取消并关闭文件", for: .normal)
        cancelButton.isHidden = active == nil && !finding
        up.isEnabled = idle && trail.count > 1; previous.isEnabled = idle && page > 0
        next.isEnabled = idle && page + 1 < pages
        replacePage(canSelect: idle, focus: focus)
    }

    private func replacePage(canSelect: Bool, focus: Int) {
        let old = table, new = UITableView()
        table = new  // A callback from the previous page has the old table, not a position in this page.
        old.delegate = nil; old.dataSource = nil; old.removeFromSuperview()
        new.register(UITableViewCell.self, forCellReuseIdentifier: "row")
        new.allowsSelection = canSelect
        new.dataSource = self; new.delegate = self
        tableHost.addArrangedSubview(new)
        new.reloadData()
        if focus >= 0, focus < new.numberOfRows(inSection: 0) {
            tableHost.layoutIfNeeded()
            new.scrollToRow(at: IndexPath(row: focus, section: 0), at: .top, animated: false)
        }
    }

    @objc private func goUp() { if mayNavigate && trail.count > 1 { trail.removeLast(); enter() } }
    @objc private func prevPage() { if mayNavigate && page > 0 { page -= 1; render() } }
    @objc private func nextPage() { if mayNavigate && (page + 1) * pageSize < children.count { page += 1; render() } }
    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        guard tableView === table, section == 0 else { return 0 }
        return min(pageSize, max(0, children.count - page * pageSize))
    }
    func tableView(_ tableView: UITableView, cellForRowAt path: IndexPath) -> UITableViewCell {
        let i = page * pageSize + path.row
        guard tableView === table, path.section == 0, children.indices.contains(i) else { return UITableViewCell() }
        let row = children[i], cell = tableView.dequeueReusableCell(withIdentifier: "row", for: path)
        var content = cell.defaultContentConfiguration()
        content.text = row.name
        content.secondaryText = row.children == nil ? "\(row.size) B（声明）" : "目录"
        cell.contentConfiguration = content
        cell.accessoryType = row.children == nil ? .none : .disclosureIndicator
        cell.selectionStyle = .default
        return cell
    }
    func tableView(_ tableView: UITableView, didSelectRowAt path: IndexPath) {
        let i = page * pageSize + path.row
        guard tableView === table, path.section == 0, mayNavigate, children.indices.contains(i),
              let view = report, selection === view.archive.scope, (try? view.archive.scope.check()) != nil else { return }
        let row = children[i]
        if row.children != nil { trail.append(row); enter() } else { offer(row) }
    }
    deinit { active?.cancel(); search?.operation.cancel(); queue.cancelAllOperations(); selection?.retire() }
}
