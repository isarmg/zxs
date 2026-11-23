import UIKit
import UniformTypeIdentifiers

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {}

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private var draftShield: UIView?
    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options: UIScene.ConnectionOptions) {
        guard let scene = scene as? UIWindowScene else { return }
        let w = UIWindow(windowScene: scene)
        w.rootViewController = PreviewController()
        self.window = w; w.makeKeyAndVisible()
    }
    func sceneWillResignActive(_ scene: UIScene) { coverDraft() }
    func sceneDidBecomeActive(_ scene: UIScene) {
        draftShield?.removeFromSuperview()
        draftShield = nil
    }
    private func coverDraft() {
        guard draftShield == nil, let window,
              let sheet = window.rootViewController?.presentedViewController as? UINavigationController,
              sheet.topViewController is LocalDraftController else { return }
        let shield = UIView(frame: window.bounds)
        shield.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        shield.backgroundColor = .systemBackground
        shield.isOpaque = true
        shield.accessibilityViewIsModal = true
        window.addSubview(shield)
        draftShield = shield
    }
    func sceneDidEnterBackground(_ scene: UIScene) {
        coverDraft()
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

@MainActor
private final class TextReading {
    weak var archive: HeldArchive?
    let member: Member, contents: PlainText.Pages?, proof: DirectoryIndex.TextIndex?
    let back = UIButton(type: .system), next = UIButton(type: .system), jump = UIButton(type: .system)
    weak var jumpAlert: UIAlertController?
    var index = 0

    init(archive: HeldArchive, member: Member, contents: PlainText.Pages?, proof: DirectoryIndex.TextIndex?) {
        self.archive = archive; self.member = member; self.contents = contents; self.proof = proof
    }
    var count: Int { proof?.count ?? contents?.count ?? 0 }
}

final class PreviewController: UIViewController, UIDocumentPickerDelegate, UIAdaptivePresentationControllerDelegate,
                               UITableViewDataSource, UITableViewDelegate {
    private let queue: OperationQueue = {
        let q = OperationQueue(); q.maxConcurrentOperationCount = 1; q.qualityOfService = .userInitiated; return q
    }()
    private let notice = UILabel(), status = UILabel(), location = UILabel(), tableHost = UIStackView()
    private var table = UITableView()
    private let cancelButton = UIButton(type: .system), up = UIButton(type: .system)
    private let checkAll = UIButton(type: .system)
    private let find = UIButton(type: .system), form = UIStackView(), query = UITextField()
    private var search: NameSearch?
    private let previous = UIButton(type: .system), next = UIButton(type: .system)
    private let directoryNav = UIStackView(), textNav = UIStackView(), literal = UITextView()
    private var textReading: TextReading?
    private var rawReading: NSObject?
    private weak var awaiting: UIDocumentPickerViewController?
    private var active: DocumentRead?
    private var selection: DocumentPresentation?, report: Report?
    private weak var decision: UIAlertController?
    private var memberConsent: NSObject?
    private var trail: [Member] = [], children: [Member] = [], page = 0
    private let pageSize = 100

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground
        for label in [notice, status, location] { label.numberOfLines = 0; label.adjustsFontForContentSizeCategory = true }
        notice.font = .preferredFont(forTextStyle: .headline)
        notice.text = "ZIP 目录预览\n打开目录只看声明。可另行核验成员、原包字节或 .zxs 整树；不认证来源。外部提供方可能下载。"
        status.font = .preferredFont(forTextStyle: .body); status.numberOfLines = 5
        location.font = .preferredFont(forTextStyle: .footnote)
        location.numberOfLines = 2
        func wire(_ button: UIButton, _ title: String, _ action: Selector) {
            button.setTitle(title, for: .normal); button.addTarget(self, action: action, for: .touchUpInside)
        }
        let pick = UIButton(type: .system)
        wire(pick, "选择 .zxs 或 Store ZIP", #selector(choose))
        let localDraft = UIButton(type: .system)
        wire(localDraft, "本地草稿…", #selector(openLocalDraft))
        wire(cancelButton, "取消并关闭文件", #selector(cancelRead))
        wire(find, "按名称查找…", #selector(offerFind))
        wire(checkAll, "核验原归档…", #selector(offerArchive))
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
        for button in [up, previous, next] { directoryNav.addArrangedSubview(button) }
        directoryNav.distribution = .fillEqually
        textNav.axis = .vertical; textNav.spacing = 3
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
        let stack = UIStackView(arrangedSubviews: [notice, pick, localDraft, find, form, checkAll, cancelButton, status, location, directoryNav,
                                                   textNav, literal, tableHost])
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
        let oldRead = active, oldScope = selection
        awaiting = nil; active = nil; selection = nil; memberConsent = nil
        // Revoke and clear text before releasing the Report's last strong held FD.
        if isViewLoaded { clearText(); clearDraft() }
        report = nil
        decision?.dismiss(animated: false); decision = nil
        retireSearch()
        oldRead?.cancel(); queue.cancelAllOperations(); oldScope?.retire()
        if isViewLoaded { reset("没有保留上次选择。") }
    }
    @objc private func cancelRead() {
        if search != nil {
            retireSearch()
            status.text = "已取消查找，保留原目录与页。"
            render()
        } else { clearSelection(); reset("已取消本次读取。") }
    }

    @objc private func openLocalDraft() {
        guard active == nil, presentedViewController == nil else { return }
        do {
            let editor = LocalDraftController(store: try LocalDraft.applicationStore())
            let sheet = UINavigationController(rootViewController: editor)
            sheet.isModalInPresentation = true
            present(sheet, animated: true)
        } catch {
            let alert = UIAlertController(title: "无法打开本地草稿", message: nil, preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "关闭", style: .cancel))
            present(alert, animated: true)
        }
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

    @objc private func offerArchive() {
        guard mayNavigate, let view = report, selection === view.archive.scope,
              (try? view.archive.checkPreview()) != nil else { return }
        let marker = view.isManagedSnapshotName ? (try? view.directory.snapshotMarker()) : nil
        let logical = marker.map { "\n.zxs 整树：读取全部成员正文 \(view.directory.bodyBytes.formatted()) 字节并复核布局。节点 \($0.operation)。" }
            ?? (view.isManagedSnapshotName ? "\n没有可识别的规范快照标记，不提供整树核验。" : "")
        clearDraft()
        let alert = UIAlertController(title: "只读核验本次原归档",
            message: "\(view.name)\n原包 SHA-256：读取原 ZIP 完整的 \(view.archive.size.formatted()) 字节，包含头及注释。"
                + logical + "\n用时取决于大小，可取消。不认证来源，不保存或上传。外部提供方可能下载。",
            preferredStyle: .alert)
        let consent = NSObject()
        alert.addAction(UIAlertAction(title: "返回目录", style: .cancel) { [weak self, weak alert] _ in
            guard let self = self, let alert = alert,
                  self.memberConsent === consent, self.decision === alert else { return }
            self.memberConsent = nil; self.decision = nil
        })
        func action(_ title: String, raw: Bool) -> UIAlertAction {
            UIAlertAction(title: title, style: .default) { [weak self, weak alert, weak origin = view.archive] _ in
                guard let self = self, let alert = alert,
                      self.memberConsent === consent, self.decision === alert else { return }
                self.memberConsent = nil; self.decision = nil
                guard let current = self.report, current.archive === origin, self.selection === current.archive.scope,
                      self.active == nil, self.search == nil, (try? current.archive.checkPreview()) != nil else { return }
                self.verifyArchive(current, raw: raw)
            }
        }
        if marker != nil { alert.addAction(action("核验 .zxs 整树", raw: false)) }
        alert.addAction(action("核对原包 SHA-256", raw: true))
        memberConsent = consent; decision = alert; present(alert, animated: true)
    }

    private func verifyArchive(_ view: Report, raw: Bool) {
        let job = DocumentRead(); active = job
        observeBody(job, archive: view.archive, total: raw ? view.archive.size : view.directory.bodyBytes,
                    title: raw ? "正在核对原包 SHA-256" : "正在核验 .zxs 整树", measure: raw ? "原包字节" : "正文")
        render()
        queue.addOperation { [weak self] in
            let outcome = Result {
                if raw { return try job.rawDigest(view) }
                let marker = try job.verifySnapshot(view)
                return "整树内容与包内标记一致。未认证来源。\n第 \(marker.generation) 代 · 节点 \(marker.operation)\n已核 \(view.directory.entryCount) 项、\(view.directory.bodyBytes) B 正文。"
            }
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.active === job, self.report?.archive === view.archive,
                      self.selection === view.archive.scope else { return }
                self.active = nil
                guard (try? view.archive.checkPreview()) != nil else { self.clearSelection(); return }
                switch outcome {
                case .success(let read):
                    if raw { self.showRaw(view, digest: read) }
                    else { self.status.text = read; self.render() }
                case .failure(let error):
                    self.clearSelection()
                    if raw {
                        self.reset("没有取得原包摘要：字节不完整、原件变化或授权失效。请重新选择。")
                    } else if case PreviewError.format = error {
                        self.reset("没有通过整树核验：ZIP 布局或实读内容与声明不符。")
                    } else {
                        self.reset("没有完整核验结果：原文件变化、权限失效或读取不可用。请重新选择。")
                    }
                }
            }
        }
    }

    private func showRaw(_ view: Report, digest: String) {
        let token = NSObject(); rawReading = token
        let close = UIButton(type: .system)
        close.setTitle("关闭原包摘要", for: .normal)
        close.addAction(UIAction { [weak self, weak token] _ in
            guard let self = self, let token = token, self.rawReading === token else { return }
            self.closePreview()
        }, for: .touchUpInside)
        status.text = "原包 SHA-256。不是源作者或版本真实性证明。"
        location.text = "原包 \(view.archive.size.formatted()) B"
        cancelButton.isHidden = true; directoryNav.isHidden = true; tableHost.isHidden = true
        checkAll.isHidden = true; find.isHidden = true; notice.isHidden = true
        literal.text = "\(view.name)\n\n\(digest)\n\n包含头、成员与 ZIP 注释。"
        literal.setContentOffset(.zero, animated: false)
        textNav.addArrangedSubview(close)
        textNav.isHidden = false; literal.isHidden = false
    }

    private func offer(_ file: Member) {
        guard mayNavigate, let view = report, selection === view.archive.scope,
              (try? view.archive.scope.check()) != nil else { return }
        clearDraft()
        let path = "/" + (trail.dropFirst().map(\.name) + [file.name]).joined(separator: "/")
        let whole = file.size <= PlainText.byteLimit
        let choice = whole ? "只有 CRC 相符且全文符合 UTF-8 显示规则时才显示全文。"
            : "按页跳读要求全文符合 UTF-8 显示规则；前部只判断开头最多 256 KiB。"
        let alert = UIAlertController(title: "查看这个归档成员",
            message: "所有选项都先读取本项全部 \(file.size) 字节。较大文件可能耗时，可以取消。\n"
                + "\(choice)\n不保存或上传。外部提供方可能自行下载。\n\(view.name)\n\(path)", preferredStyle: .alert)
        let consent = NSObject()
        func action(_ title: String, mode: MemberReadMode) -> UIAlertAction {
            UIAlertAction(title: title, style: .default) { [weak self, weak alert, weak archive = view.archive] _ in
                guard let self = self, let alert = alert,
                      self.memberConsent === consent, self.decision === alert else { return }
                self.memberConsent = nil; self.decision = nil
                guard let current = self.report, current.archive === archive, self.active == nil,
                      self.showing(current, file: file), (try? current.archive.scope.check()) != nil else { return }
                self.verify(current, file: file, path: path, mode: mode)
            }
        }
        alert.addAction(UIAlertAction(title: "返回目录", style: .cancel) { [weak self, weak alert] _ in
            guard let self = self, let alert = alert,
                  self.memberConsent === consent, self.decision === alert else { return }
            self.memberConsent = nil; self.decision = nil
        })
        alert.addAction(action("只核验 CRC", mode: .crc))
        alert.addAction(action(whole ? "核验后看全文" : "完整核验后预览前部", mode: .preview))
        if !whole { alert.addAction(action("完整核验后按页跳读全文", mode: .indexed)) }
        memberConsent = consent; decision = alert; present(alert, animated: true)
    }
    private func showing(_ record: Report, file: Member) -> Bool {
        guard search == nil, selection === record.archive.scope, report?.archive === record.archive else { return false }
        let first = page * pageSize
        return first < children.count && children[first..<min(first + pageSize, children.count)].contains { $0 === file }
    }
    private func observeBody(_ job: DocumentRead, archive: HeldArchive, total: Int,
                             title: String, measure: String = "正文", detail: String = "", previous: Int = -1) {
        guard active === job, report?.archive === archive, selection === archive.scope,
              (try? archive.scope.check()) != nil else { return }
        let seen = job.bytesProcessed
        if seen != previous {
            let ending = seen == total ? "；仍在核验。" : "。"
            status.text = "\(title)，可取消。\n已处理\(measure) \(seen.formatted()) / \(total.formatted()) B\(ending)\(detail)"
        }
        // One queued redraw at most for this live read. Revocation or a terminal result
        // consumes active, and the weak callback can't keep a document or read alive.
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(350)) { [weak self, weak job, weak archive] in
            guard let job = job, let archive = archive else { return }
            self?.observeBody(job, archive: archive, total: total, title: title, measure: measure, detail: detail, previous: seen)
        }
    }
    private func verify(_ view: Report, file: Member, path: String, mode: MemberReadMode) {
        let job = DocumentRead(); active = job
        let hint = mode == .crc ? "" : mode == .indexed ? "\n同时核验全文与定位索引"
            : file.size <= PlainText.byteLimit ? "\nCRC 相符后显示全文" : "\nCRC 相符后只显示前部"
        observeBody(job, archive: view.archive, total: file.size,
                    title: "正在核验所选成员", detail: hint + "\n" + path)
        render()
        queue.addOperation { [weak self] in
            let outcome = Result { try job.verify(view, file: file, mode: mode) }
            DispatchQueue.main.async { [weak self] in
                guard let self = self, self.active === job, self.selection === view.archive.scope else { return }
                self.active = nil
                guard self.showing(view, file: file), (try? view.archive.scope.check()) != nil else { self.clearSelection(); return }
                switch outcome {
                case .success(let result):
                    let verdict = result.matches ? "该项 CRC-32 与 ZIP 声明相符。" : "该项 CRC-32 与 ZIP 声明不符。"
                    let values = String(format: "声明 %08X，实读 %08X。", result.declared, result.actual)
                    let show = result.text != nil || result.indexed != nil
                    let plainNotice = mode != .crc && result.matches && !show
                        ? (mode == .indexed ? "\n全文不符合 UTF-8 显示规则，不提供跳读。"
                                            : "\n本次预览范围不符合 UTF-8 显示规则，不显示正文。") : ""
                    self.status.text = "\(verdict)\n\(values) 不是签名，不证明整包或其他项。\(plainNotice)\n\(path)（\(file.size) B）"
                    self.render()
                    if mode != .crc && result.matches && show { self.showText(result, view: view, file: file) }
                case .failure:
                    self.clearSelection()
                    self.reset("没有完整核验结果：布局不符、原文件变化或读取不可用。请重新选择。")
                }
            }
        }
    }
    private func showText(_ result: MemberCRC, view: Report, file: Member) {
        let reading = TextReading(archive: view.archive, member: file, contents: result.text, proof: result.indexed)
        textReading = reading
        func wire(_ button: UIButton, _ title: String, _ action: @escaping (PreviewController, TextReading) -> Void) {
            button.setTitle(title, for: .normal)
            button.titleLabel?.adjustsFontForContentSizeCategory = true
            button.titleLabel?.numberOfLines = 0; button.titleLabel?.font = .preferredFont(forTextStyle: .body)
            // Each action belongs to the original reading, never to whichever page is visible later.
            button.addAction(UIAction { [weak self, weak reading] _ in
                guard let self = self, let reading = reading, self.textReading === reading else { return }
                action(self, reading)
            }, for: .touchUpInside)
        }
        func row(_ buttons: [UIButton]) {
            let bar = UIStackView(arrangedSubviews: buttons)
            bar.distribution = .fillEqually
            textNav.addArrangedSubview(bar)
        }
        wire(reading.back, "上一文本页") { $0.showTextPage($1, $1.index - 1) }
        wire(reading.next, "下一文本页") { $0.showTextPage($1, $1.index + 1) }
        let close = UIButton(type: .system)
        wire(close, "关闭文本") { view, _ in view.closePreview() }
        if reading.proof != nil {
            wire(reading.jump, "跳到页码…") { $0.offerJump($1) }
            row([reading.back, reading.next]); row([reading.jump, close])
        } else { row([reading.back, reading.next, close]) }
        status.numberOfLines = 3; tableHost.isHidden = true; directoryNav.isHidden = true
        find.isHidden = true; checkAll.isHidden = true; notice.isHidden = true
        literal.isHidden = false; textNav.isHidden = false
        showTextPage(reading, 0)
    }
    private func displayedReport(_ reading: TextReading) -> Report? {
        guard textReading === reading, active == nil, let view = report, view.archive === reading.archive,
              showing(view, file: reading.member), (try? view.archive.checkPreview()) != nil else { return nil }
        return view
    }
    private func unavailableText() {
        clearSelection()
        reset("无法复核原件、授权或本页字节，已关闭文本。请重新选择。")
    }
    private func offerJump(_ reading: TextReading) {
        guard textReading === reading, reading.proof != nil, active == nil,
              presentedViewController == nil else { return }
        guard displayedReport(reading) != nil else { unavailableText(); return }
        let alert = UIAlertController(title: "跳到页码", message: "输入 1 至 \(reading.count) 之间的页码。",
                                      preferredStyle: .alert)
        alert.addTextField { field in
            field.placeholder = "页码（0–9）"; field.keyboardType = .numberPad
            field.autocorrectionType = .no; field.spellCheckingType = .no
            field.accessibilityLabel = "文本页码"
        }
        alert.addAction(UIAlertAction(title: "取消", style: .cancel) { [weak self, weak reading, weak alert] _ in
            guard let self = self, let reading = reading, let alert = alert,
                  self.textReading === reading, self.decision === alert else { return }
            alert.textFields?.first?.text = nil
            self.decision = nil; reading.jumpAlert = nil
        })
        alert.addAction(UIAlertAction(title: "跳转", style: .default) { [weak self, weak reading, weak alert] _ in
            guard let self = self, let reading = reading, let alert = alert,
                  self.textReading === reading, self.decision === alert else { return }
            let input = alert.textFields?.first?.text ?? ""
            alert.textFields?.first?.text = nil
            self.decision = nil; reading.jumpAlert = nil
            guard self.displayedReport(reading) != nil else { self.unavailableText(); return }
            let digits = input.utf8.prefix(10)
            guard !digits.isEmpty, digits.count <= 9, digits.allSatisfy({ (48...57).contains($0) }),
                  let number = Int(input), (1...reading.count).contains(number) else {
                self.location.text = "页码不在 1–\(reading.count) 内。仍在第 \(reading.index + 1) 页。"; return
            }
            self.showTextPage(reading, number - 1)
        })
        reading.jumpAlert = alert; decision = alert
        present(alert, animated: true)
    }
    private func navigation(_ reading: TextReading, ready: Bool) {
        reading.back.isEnabled = ready && reading.index > 0
        reading.next.isEnabled = ready && reading.index + 1 < reading.count
        reading.jump.isEnabled = ready
    }
    private func drawTextPage(_ reading: TextReading, _ index: Int, _ body: String) {
        guard textReading === reading else { return }
        guard displayedReport(reading) != nil else { unavailableText(); return }
        reading.index = index
        let text = reading.contents
        let kind = text.map { $0.complete ? ($0.totalBytes == 0 ? "空文本" : "核验后的全文") : "核验后仅看前部" }
            ?? "核验后按页跳读"
        let portion = text.map { $0.complete ? "" : "，只显示前 \($0.displayedBytes) B" } ?? ""
        location.text = "\(kind) · \(index + 1)/\(reading.count) 页\n已核全部 \(reading.member.size) B\(portion)"
        literal.text = body // only this bounded page; never links, markup or an external-open action
        literal.setContentOffset(.zero, animated: false)
        navigation(reading, ready: true)
    }
    private func showTextPage(_ reading: TextReading, _ index: Int) {
        guard textReading === reading, active == nil, (0..<reading.count).contains(index) else { return }
        if let small = reading.contents { drawTextPage(reading, index, small.page(index)); return }
        guard let proof = reading.proof else { return }
        guard let view = displayedReport(reading) else { unavailableText(); return }
        let job = DocumentRead(), file = reading.member
        active = job
        literal.text = nil; navigation(reading, ready: false)
        location.text = "正在复核第 \(index + 1)/\(reading.count) 页；关闭文本可取消。"
        // The worker keeps the original grant/FD alive only until the coordinated read drains.
        queue.addOperation { [weak self, weak reading] in
            let outcome = Result { try job.page(view, file: file, proof: proof, index: index) }
            DispatchQueue.main.async { [weak self, weak reading] in
                guard let self = self, let reading = reading,
                      self.textReading === reading, self.active === job else { return }
                self.active = nil
                switch outcome {
                case .success(let body): self.drawTextPage(reading, index, body)
                case .failure: self.unavailableText()
                }
            }
        }
    }
    private func clearText() {
        let previous = textReading; textReading = nil
        rawReading = nil
        literal.text = nil; literal.isHidden = true; textNav.isHidden = true
        let page = previous == nil ? nil : active
        if previous != nil { active = nil } // only a page job can coexist with this reading
        if let alert = previous?.jumpAlert {
            alert.textFields?.first?.text = nil
            if decision === alert { decision = nil; alert.dismiss(animated: false) }
        }
        page?.cancel()
        for child in textNav.arrangedSubviews { child.removeFromSuperview() }
        tableHost.isHidden = false; directoryNav.isHidden = false; notice.isHidden = false; find.isHidden = false
        status.numberOfLines = 5
    }
    private func closePreview() {
        clearText()
        if let report = report, selection === report.archive.scope, (try? report.archive.checkPreview()) != nil {
            render()
        } else { clearSelection() }
    }
    private func reset(_ message: String) {
        clearDraft(); clearText()
        trail = []; children = []; page = 0
        status.text = message; location.text = nil; cancelButton.isHidden = active == nil
        cancelButton.setTitle("取消并关闭文件", for: .normal)
        find.isEnabled = false; checkAll.isHidden = true
        up.isEnabled = false; previous.isEnabled = false; next.isEnabled = false
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
        checkAll.isHidden = report == nil || !literal.isHidden
        checkAll.isEnabled = idle && !checkAll.isHidden
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
