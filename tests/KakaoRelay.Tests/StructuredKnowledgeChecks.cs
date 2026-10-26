using KakaoRelay.Core;

internal static class StructuredKnowledgeChecks
{
    public static async Task RunAsync(string root, Action<bool, string> check)
    {
        var vault = Path.Combine(root, "structured-vault"); Directory.CreateDirectory(vault);
        StructuredKnowledge.Ensure(vault);
        var room = new KnowledgeRoomSubscription("fixture", "room", "Fixture", Guid.NewGuid());
        var now = DateTimeOffset.UtcNow;
        LocalMessage Message(string id, string text) => new(id, "person", "Fixture", now, 1, text, false);
        var first = new KnowledgeClaim("출시 준비", "OLDSCHEDULE42 금요일 출시 준비", "task", "confirmed", [new("1", "금요일")],
            Project: "테스트 프로젝트", Owner: "담당A", Due: "금요일", TaskState: "todo", Tags: [" 배포 ", "배포", "CI CD"]);
        var plan = StructuredKnowledge.Plan(vault, room, new([first]), [Message("1", "금요일")], now, "batch-1", true);
        StructuredKnowledge.Apply(vault, plan.Writes);
        var initial = StructuredKnowledge.Load(vault).Entities.Values.Single();
        var path = Path.Combine(vault, initial.Source);
        check(plan.Saved == 1 && initial.Owner == "담당A" && initial.Due == "금요일" && initial.TaskState == "todo"
            && File.ReadAllText(path).StartsWith("---\n") && File.ReadAllText(path).Contains("[ ] 출시 준비"),
            "Task notes expose Obsidian frontmatter, owner, due date, stable identity and checkbox state");
        var projectPath = Directory.GetFiles(Path.Combine(vault, "01-프로젝트"), "프로젝트-*.md").Single();
        check(File.ReadAllText(projectPath).Contains("출시 준비") && File.ReadAllText(projectPath).Contains("03-할일/"),
            "Project dashboards link their current tasks and show present metadata");
        check(initial.Tags.Contains("유형/할일") && initial.Tags.Contains("프로젝트/테스트-프로젝트") && initial.Tags.Contains("상태/예정")
            && initial.Tags.Count(t => t == "주제/배포") == 1 && initial.Tags.Contains("주제/ci-cd") && File.ReadAllText(path).Contains("title:"),
            "Tags normalize topic variants while consistent type, project, state and human-readable titles are generated");
        File.AppendAllText(path, "사용자 직접 메모를 보존하세요.\n");
        var update = first with { Topic = "출시 준비 완료", Text = "NEWSCHEDULE42 월요일 출시 준비 완료", Relation = "update", EntityId = initial.Id,
            RelatedSource = initial.Source, RelatedQuote = initial.Text, Due = "월요일", TaskState = "done", Owner = null, Evidence = [new("2", "월요일 완료 확정")] };
        var references = new List<KnowledgeExcerpt> { new(initial.Source, 1, File.ReadAllText(path)) };
        AutoKnowledgeService.ValidateDigest(new([update]), [Message("2", "월요일 완료 확정")], references);
        var changed = StructuredKnowledge.Plan(vault, room, new([update]), [Message("2", "월요일 완료 확정")], now.AddMinutes(1), "batch-2", true);
        // Recover a crash after the first file has already been committed.
        StructuredKnowledge.Apply(vault, changed.Writes.Take(1).ToList());
        StructuredKnowledge.Apply(vault, changed.Writes);
        StructuredKnowledge.Apply(vault, changed.Writes);
        var current = StructuredKnowledge.Load(vault).Entities.Values.Single();
        check(current.Id == initial.Id && current.Source == initial.Source && current.Topic == update.Topic && current.Owner == "담당A"
            && current.TaskState == "done" && File.ReadAllText(path).Contains("사용자 직접 메모") && File.ReadAllText(path).Contains("[x]"),
            "Explicit updates retain identity, filename, unspecified owner and user notes while updating current task state");
        check(current.Tags.Contains("상태/완료") && !current.Tags.Contains("상태/예정") && current.Aliases.Contains(initial.Topic),
            "Renamed notes retain their previous title as an alias and replace stale status tags");
        check(Directory.GetFiles(Path.Combine(vault, "05-변경이력"), "*.md").Length == 1
            && File.ReadAllText(Directory.GetFiles(Path.Combine(vault, "05-변경이력"), "*.md").Single()).Contains(initial.Text),
            "Interrupted publication recovers idempotently and retains the previous fact and evidence once");
        var settings = new KnowledgeSettings { Enabled = true, VaultPath = vault };
        check((await ObsidianKnowledge.SearchAsync(settings, "OLDSCHEDULE42")).Excerpts.Count == 0
            && (await ObsidianKnowledge.SearchAsync(settings, "NEWSCHEDULE42")).Excerpts.Count > 0,
            "Bot retrieval uses current knowledge and excludes stale conversation and change archives");
        check((await KnowledgeDocuments.ListAsync(vault)).Notes.All(n => !StructuredKnowledge.Historical(n.Source)),
            "The in-app library shows current notes rather than historical duplicates");

        var conflict = update with { Text = "확인되지 않은 일정", Relation = "conflict", RelatedQuote = current.Text, Evidence = [new("3", "아마")] };
        var held = StructuredKnowledge.Plan(vault, room, new([conflict]), [Message("3", "아마")], now, "batch-3", true);
        StructuredKnowledge.Apply(vault, held.Writes);
        check(held.Review == 1 && held.Saved == 0 && StructuredKnowledge.Load(vault).Entities[current.Id].Text == current.Text,
            "Unconfirmed contradictions go to review without replacing the current fact");
        var badUpdate = update with { EntityId = new string('A', 32), RelatedQuote = current.Text };
        var invalid = StructuredKnowledge.Plan(vault, room, new([badUpdate]), [Message("2", "월요일 완료 확정")], now, "batch-4", true);
        check(invalid.Review == 1 && invalid.Saved == 0, "Unknown update IDs never create or overwrite a canonical note");

        var retag = update with { Tags = ["릴리스"], RelatedQuote = current.Text };
        var retagged = StructuredKnowledge.Plan(vault, room, new([retag]), [Message("2", "월요일 완료 확정")], now, "retag", true);
        StructuredKnowledge.Apply(vault, retagged.Writes);
        check(retagged.Saved == 1 && StructuredKnowledge.Load(vault).Entities[current.Id].Tags.Contains("주제/릴리스")
            && File.ReadAllText(path).Contains("\"주제/릴리스\""),
            "Verified metadata-only updates replace topic tags and write readable Korean Obsidian properties");

        var next = update with { Text = "새 상태", RelatedQuote = "월요일 완료 확정" };
        var before = File.ReadAllText(path);
        File.WriteAllText(path, before.Replace(current.Text, "사람이 직접 수정한 관리 내용"));
        try { StructuredKnowledge.Plan(vault, room, new([next]), [Message("2", "월요일 완료 확정")], now, "manual", true); check(false, "Manual managed edits"); }
        catch (IOException) { check(File.ReadAllText(path).Contains("사람이 직접 수정한 관리 내용"), "Manual changes inside managed content are preserved rather than overwritten"); }
        File.WriteAllText(path, before);
        var race = StructuredKnowledge.Plan(vault, room, new([next]), [Message("2", "월요일 완료 확정")], now, "race", true);
        File.AppendAllText(path, "편집 경합");
        try { StructuredKnowledge.Apply(vault, race.Writes); check(false, "Concurrent edit"); }
        catch (IOException) { check(!File.Exists(Path.Combine(vault, "05-변경이력", initial.Id + "-race.md")), "A changed target stops the whole transaction before any overwrite"); }
        try { StructuredKnowledge.Apply(vault, [new("../outside.md", null, "bad")]); check(false, "Traversal"); }
        catch (IOException) { check(!File.Exists(Path.Combine(root, "outside.md")), "Publication rejects paths outside the vault"); }

        var migration = Path.Combine(root, "migration-vault"); Directory.CreateDirectory(Path.Combine(migration, "자동정리"));
        var original = "# Legacy 대화 자동 정리\n\n계정: fixture\n대화방 ID: legacy\n\n## 업무 규칙\n\nLEGACYFACT42 규칙입니다.\n\n> 종류: knowledge · 대화에서 확인된 내용 (외부 사실 검증 아님)\n\n- 근거: 메시지 5 · 원래 시각 · 작성자\n  > 실제 근거\n";
        var oldFile = Path.Combine(migration, "자동정리", "legacy.md"); File.WriteAllText(oldFile, original);
        StructuredKnowledge.Ensure(migration); StructuredKnowledge.Ensure(migration);
        check(File.ReadAllText(oldFile) == original && StructuredKnowledge.Load(migration).Entities.Count == 1
            && StructuredKnowledge.Load(migration).Migrated.Contains("자동정리/legacy.md"),
            "Legacy summaries migrate once into separate entities while their original files remain byte-for-byte intact");
        var migratedSearch = await ObsidianKnowledge.SearchAsync(new() { Enabled = true, VaultPath = migration }, "LEGACYFACT42");
        check(migratedSearch.Excerpts.Count > 0 && migratedSearch.Excerpts.All(e => !e.Source.StartsWith("자동정리/")),
            "Migrated legacy summaries cannot compete with their current structured notes in retrieval");
    }
}
