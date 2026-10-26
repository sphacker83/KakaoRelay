using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace KakaoRelay.Core;

public sealed record KnowledgeWrite(string Source, string? Before, string Body);
public sealed record KnowledgePlan(List<KnowledgeWrite> Writes, int Saved, int Review, int Duplicates);
public sealed record KnowledgeEntity(string Id, string ProjectId, string Project, string Topic, string Kind, string Text,
    string? Owner, string? Due, string? TaskState, string Source, string Profile, string RoomId, string RoomTitle,
    string Evidence, DateTimeOffset CreatedAt, DateTimeOffset UpdatedAt)
{
    public List<string> Changes { get; init; } = [];
    public List<string> Tags { get; init; } = [];
    public List<string> Aliases { get; init; } = [];
}

// The hidden index owns identities, while Markdown stays readable and editable in Obsidian.
public static class StructuredKnowledge
{
    private const string IndexPath = ".kakaorelay/structured.json";
    private const string RecoveryPath = ".kakaorelay/migration.pending.json";
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented = true, Encoder = PromptJson.Encoder };
    private static readonly JsonSerializerOptions InlineJson = new() { Encoder = PromptJson.Encoder };
    private static string Property<T>(T value) => JsonSerializer.Serialize(value, InlineJson);
    private static string Header(string title, string type, IEnumerable<string> tags, DateTimeOffset now) =>
        $"---\ntitle: {Property(title)}\ntype: {type}\ntags: {Property(tags)}\ncreated: {now:O}\n---\n";
    public sealed class Catalog
    {
        public int Version { get; set; } = 1;
        public Dictionary<string, KnowledgeEntity> Entities { get; set; } = [];
        public Dictionary<string, string> Blocks { get; set; } = [];
        public HashSet<string> Migrated { get; set; } = [];
    }
    public static Catalog Load(string vault)
    {
        var path = SafePath(vault, IndexPath);
        return File.Exists(path) ? JsonSerializer.Deserialize<Catalog>(File.ReadAllText(path), Json) ?? throw new IOException("구조화 지식 색인을 읽을 수 없습니다.") : new();
    }
    private static string Key(string value) => Regex.Replace(value.Normalize().ToLowerInvariant(), @"\s+", " ").Trim();
    private static string Id(string value) => AiConversation.Hash(value)[..32];
    private static string Tag(string value) => Regex.Replace(Regex.Replace(value.Normalize().Trim().TrimStart('#').ToLowerInvariant(), @"\s+", "-"), @"[^\p{L}\p{N}_-]", "").Trim('-');
    private static List<string> Tags(string kind, string project, string? state, IEnumerable<string> topics)
    {
        var type = kind switch { "project" => "프로젝트", "decision" => "결정", "task" => "할일", _ => "업무지식" };
        var status = state switch { "todo" => "예정", "doing" => "진행중", "blocked" => "보류", "done" => "완료", "cancelled" => "취소", _ => kind == "task" ? "미정" : "확정" };
        return new[] { "유형/" + type, "프로젝트/" + Tag(project), "상태/" + status }
            .Concat(topics.Select(t => "주제/" + Tag(t.Split('/').Last())).Where(t => t.Length > 3)).Distinct(StringComparer.Ordinal).ToList();
    }
    private static string Heading(string value) => value.Replace('\r', ' ').Replace('\n', ' ').Replace("[", "(").Replace("]", ")").Replace("|", " ");
    private static string Slug(string value) => Regex.Replace(Heading(value), @"[^\p{L}\p{N} _-]", "_")[..Math.Min(Heading(value).Length, 48)].Trim().TrimEnd('.');
    private static string Link(string source, string title) => $"[[{source[..^3]}|{Heading(title)}]]";
    private static string Wrap(string id, string body)
    {
        var split = body.StartsWith("---\n", StringComparison.Ordinal) ? body.IndexOf("\n---\n", 4, StringComparison.Ordinal) : -1;
        var metadata = split < 0 ? "" : body[..(split + 5)];
        var content = split < 0 ? body : body[(split + 5)..];
        return $"{metadata}<!-- kakaorelay:{id}:start -->\n{content.Trim()}\n<!-- kakaorelay:{id}:end -->";
    }
    public static bool Historical(string source) => source.StartsWith("04-대화기록/", StringComparison.Ordinal) || source.StartsWith("05-변경이력/", StringComparison.Ordinal);
    public static bool Current(string source) => source.StartsWith("01-프로젝트/", StringComparison.Ordinal) || source.StartsWith("02-업무지식/", StringComparison.Ordinal) || source.StartsWith("03-할일/", StringComparison.Ordinal);

    public static KnowledgePlan Plan(string vault, KnowledgeRoomSubscription room, KnowledgeDigest digest, List<LocalMessage> messages,
        DateTimeOffset now, string batch, bool autoSave, string? legacySource = null, string? legacyBody = null)
    {
        var catalog = Load(vault);
        var writes = new List<KnowledgeWrite>();
        var changedProjects = new HashSet<string>();
        var review = new StringBuilder(); var journal = new StringBuilder();
        int saved = 0, reviews = 0, duplicates = 0;
        foreach (var claim in digest.Items)
        {
            if (claim.Relation == "duplicate") { duplicates++; continue; }
            var project = string.IsNullOrWhiteSpace(claim.Project) ? "미분류 · " + room.Title : claim.Project.Trim();
            var projectId = Id(string.IsNullOrWhiteSpace(claim.Project) ? "room:" + room.Profile + ":" + room.RoomId : "project:" + Key(project));
            var entityId = claim.EntityId ?? Id(projectId + ":" + claim.Kind + ":" + Key(claim.Topic));
            catalog.Entities.TryGetValue(entityId, out var old);
            var evidence = legacySource is not null ? $"- 이전 요약: {Link(legacySource, "원본 대화 요약")}\n\n" + Regex.Match(legacyBody ?? "", @"(?ms)^## " + Regex.Escape(claim.Topic) + @"\r?\n.*?(?=^## |\z)").Value : string.Join("\n", claim.Evidence.Select(e =>
            {
                var m = messages.Single(m => m.Id == e.MessageId);
                return $"- 근거: 메시지 {m.Id} · {m.Time:O} · {Heading(m.Author)}\n  > {e.Quote.Replace("\n", "\n  > ")}";
            }));
            var owner = claim.Owner ?? old?.Owner; var due = claim.Due ?? old?.Due; var taskState = claim.TaskState ?? old?.TaskState;
            var tags = Tags(claim.Kind, old?.Project ?? project, taskState, claim.Tags is { Count: > 0 } ? claim.Tags : old is { Tags.Count: > 0 } ? old.Tags.Where(t => t.StartsWith("주제/", StringComparison.Ordinal)) : [claim.Topic]);
            var unchanged = old is not null && Key(old.Text) == Key(claim.Text) && old.Owner == owner && old.Due == due && old.TaskState == taskState
                && old.Topic == claim.Topic && old.Tags.Order().SequenceEqual(tags.Order());
            if (unchanged) { duplicates++; continue; }
            var validUpdate = old is not null && claim.Relation == "update" && claim.EntityId == old.Id && claim.RelatedSource == old.Source
                && claim.Kind == old.Kind && !string.IsNullOrWhiteSpace(claim.RelatedQuote) && Read(vault, old.Source).Contains(claim.RelatedQuote, StringComparison.Ordinal);
            var conflict = claim.Relation == "conflict" || (old is not null && !validUpdate) || (claim.Relation == "update" && !validUpdate)
                || (claim.EntityId is not null && old is null);
            if (!autoSave || claim.Status != "confirmed" || conflict)
            {
                reviews++;
                review.AppendLine($"## {Heading(claim.Topic)}\n\n{claim.Text}\n\n프로젝트: {Heading(project)} · 종류: {claim.Kind}\n검토 사유: {(conflict ? "충돌 또는 갱신 대상 확인 필요" : "미확정 또는 자동 저장 꺼짐")}\n");
                if (claim.RelatedSource is not null) review.AppendLine($"비교 노트: {claim.RelatedSource}\n> {claim.RelatedQuote}\n");
                review.AppendLine(evidence).AppendLine();
                continue;
            }
            if (old is not null) { project = old.Project; projectId = old.ProjectId; }
            var folder = claim.Kind == "task" ? "03-할일" : claim.Kind == "knowledge" ? "02-업무지식" : "01-프로젝트";
            var source = old?.Source ?? $"{folder}/{Slug(project)}-{Slug(claim.Topic)}-{entityId[..8]}.md";
            var entity = new KnowledgeEntity(entityId, projectId, project, claim.Topic, claim.Kind, claim.Text,
                owner, due, taskState, source, room.Profile, room.RoomId, room.Title, evidence, old?.CreatedAt ?? now, now)
            {
                Tags = tags,
                Aliases = old is null ? [] : old.Aliases.Append(old.Topic).Where(t => t != claim.Topic).Distinct().ToList()
            };
            if (old is not null)
            {
                var history = $"05-변경이력/{entityId}-{batch}.md";
                entity = entity with { Changes = [.. old.Changes, history] };
                AddRaw(history, Header(claim.Topic + " 변경 이력", "change-log", ["유형/변경이력", "프로젝트/" + Tag(project)], now)
                    + $"# {Heading(claim.Topic)} 변경 이력\n\n시각: {now:O}\n현재 노트: {Link(source, claim.Topic)}\n\n## 변경 전\n\n제목: {Heading(old.Topic)}\n태그: {string.Join(", ", old.Tags)}\n\n{old.Text}\n\n담당자: {old.Owner} · 기한: {old.Due} · 상태: {old.TaskState}\n\n{old.Evidence}\n\n## 변경 후\n\n제목: {Heading(entity.Topic)}\n태그: {string.Join(", ", entity.Tags)}\n\n{entity.Text}\n\n담당자: {entity.Owner} · 기한: {entity.Due} · 상태: {entity.TaskState}\n\n{evidence}\n");
            }
            catalog.Entities[entityId] = entity;
            Managed(source, entityId, Render(entity));
            changedProjects.Add(projectId); saved++;
            journal.AppendLine($"- {(old is null ? "추가" : "갱신")}: {Link(source, claim.Topic)}\n\n  {claim.Text}\n\n{evidence}\n");
        }
        foreach (var projectId in changedProjects)
        {
            var entities = catalog.Entities.Values.Where(e => e.ProjectId == projectId).OrderBy(e => e.Kind).ThenBy(e => e.Topic).ToList();
            var project = entities[0].Project;
            var source = $"01-프로젝트/프로젝트-{Slug(project)}-{projectId[..8]}.md";
            var body = new StringBuilder($"---\nid: {projectId}\ntype: project-index\ntitle: {Property(project)}\ntags: {Property(new[] { "유형/프로젝트", "프로젝트/" + Tag(project) })}\nproject: {Property(project)}\nupdated: {now:O}\n---\n# {Heading(project)}\n");
            foreach (var category in new[] { ("project", "목표·현재 상태"), ("decision", "결정사항"), ("task", "할 일"), ("knowledge", "업무 지식") })
            {
                body.AppendLine($"\n## {category.Item2}\n");
                foreach (var e in entities.Where(e => e.Kind == category.Item1))
                    body.AppendLine($"- {(e.Kind == "task" ? e.TaskState is "done" or "cancelled" ? "[x] " : "[ ] " : "")}{Link(e.Source, e.Topic)} — {Heading(e.Text)}{(e.Kind == "task" ? $" · {e.Owner ?? "담당 미정"} · {e.Due ?? "기한 미정"} · {e.TaskState ?? "상태 미정"}" : "")}");
            }
            Managed(source, "project-" + projectId, body.ToString());
        }
        if (saved > 0) AddRaw($"04-대화기록/{now:yyyy-MM-dd}-{Slug(room.Title)}-{batch}.md", Header($"{now:yyyy-MM-dd} {room.Title} 대화 정리", "conversation-log", ["유형/대화기록"], now) + $"# {Heading(room.Title)} 대화 정리\n\n시각: {now:O}\n계정: {room.Profile}\n방 ID: {room.RoomId}\n\n{journal}");
        if (reviews > 0) AddRaw($"검토필요/{batch}.md", Header($"{room.Title} 검토할 항목", "review", ["유형/검토", "상태/검토필요"], now) + $"# {Heading(room.Title)} · 검토할 항목\n\n> 봇 검색에서 제외됩니다.\n\n{review}");
        if (legacySource is not null) catalog.Migrated.Add(legacySource);
        if (writes.Count > 0 || legacySource is not null) AddRaw(IndexPath, JsonSerializer.Serialize(catalog, Json));
        return new(writes, saved, reviews, duplicates);

        void AddRaw(string source, string body)
        {
            var existing = writes.FindIndex(w => w.Source == source);
            var before = existing >= 0 ? writes[existing].Before : HashOrNull(vault, source);
            var write = new KnowledgeWrite(source, before, body);
            if (existing >= 0) writes[existing] = write; else writes.Add(write);
        }
        void Managed(string source, string id, string body)
        {
            var block = Wrap(id, body);
            var prior = writes.LastOrDefault(w => w.Source == source)?.Body ?? Read(vault, source);
            var start = $"<!-- kakaorelay:{id}:start -->"; var end = $"<!-- kakaorelay:{id}:end -->";
            if (prior.Length != 0)
            {
                var a = prior.IndexOf(start, StringComparison.Ordinal); var b = prior.IndexOf(end, StringComparison.Ordinal);
                if (a >= 0 && prior.StartsWith("---\n", StringComparison.Ordinal)) a = 0;
                if (a < 0 || b < a || !catalog.Blocks.TryGetValue(source, out var expected) || AiConversation.Hash(prior[a..(b + end.Length)]) != expected)
                    throw new IOException("자동 관리 영역이 수동 수정되었습니다. 기존 내용을 보존했습니다: " + source);
                block = prior[..a] + block + prior[(b + end.Length)..];
            }
            else block += "\n\n## 직접 메모\n\n";
            catalog.Blocks[source] = AiConversation.Hash(Wrap(id, body));
            AddRaw(source, block);
        }
    }

    private static string Render(KnowledgeEntity e) => $"""
        ---
        id: {e.Id}
        title: {Property(e.Topic)}
        aliases: {Property(e.Aliases)}
        type: {e.Kind}
        tags: {Property(e.Tags)}
        project: {Property(e.Project)}
        owner: {Property(e.Owner)}
        due: {Property(e.Due)}
        status: {e.TaskState ?? "confirmed"}
        source_room: {Property(e.RoomTitle)}
        source_room_id: {Property(e.RoomId)}
        source_profile: {Property(e.Profile)}
        created: {e.CreatedAt:O}
        updated: {e.UpdatedAt:O}
        ---
        # {Heading(e.Topic)}

        ## 현재 내용

        {e.Text}

        {(e.Kind == "task" ? $"- [{(e.TaskState is "done" or "cancelled" ? "x" : " ")}] {Heading(e.Topic)}\n담당자: {e.Owner ?? "미정"} · 기한: {e.Due ?? "미정"} · 상태: {e.TaskState ?? "미정"}" : "")}

        프로젝트: {Link($"01-프로젝트/프로젝트-{Slug(e.Project)}-{e.ProjectId[..8]}.md", e.Project)}

        ## 근거

        {e.Evidence}

        ## 변경 이력

        {(e.Changes.Count == 0 ? "아직 변경 이력이 없습니다." : string.Join("\n", e.Changes.Select((source, index) => "- " + Link(source, $"변경 {index + 1}"))))}
        """;

    public static void Apply(string vault, IReadOnlyList<KnowledgeWrite> writes)
    {
        // Validate the whole plan first; repeated application also accepts already-written content.
        foreach (var write in writes) Validate(write);
        foreach (var write in writes)
        {
            var path = SafePath(vault, write.Source);
            if (HashOrNull(vault, write.Source) == AiConversation.Hash(write.Body)) continue;
            Validate(write);
            var parent = Path.GetDirectoryName(path)!; Directory.CreateDirectory(parent); ObsidianKnowledge.CheckDirectory(parent);
            var temp = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            File.WriteAllText(temp, write.Body, new UTF8Encoding(false));
            File.Move(temp, path, true);
        }
        void Validate(KnowledgeWrite write)
        {
            if (write.Source != IndexPath && !new[] { "01-프로젝트/", "02-업무지식/", "03-할일/", "04-대화기록/", "05-변경이력/", "검토필요/" }.Any(prefix => write.Source.StartsWith(prefix, StringComparison.Ordinal)))
                throw new IOException("자동 관리 경로가 아닙니다.");
            var current = HashOrNull(vault, write.Source);
            if (current != write.Before && current != AiConversation.Hash(write.Body)) throw new IOException("저장 도중 노트가 수정되어 덮어쓰지 않았습니다: " + write.Source);
            if (write.Source.EndsWith(".md", StringComparison.Ordinal) && Encoding.UTF8.GetByteCount(write.Body) > ObsidianKnowledge.MaxFileBytes)
                throw new IOException("관리 노트가 너무 큽니다. 프로젝트를 나누어 주세요: " + write.Source);
        }
    }
    private static string? HashOrNull(string vault, string source) => File.Exists(SafePath(vault, source)) ? AiConversation.Hash(Read(vault, source)) : null;
    private static string Read(string vault, string source) { var path = SafePath(vault, source); return File.Exists(path) ? File.ReadAllText(path) : ""; }
    private static string SafePath(string vault, string source)
    {
        var root = Path.GetFullPath(vault); ObsidianKnowledge.CheckDirectory(root);
        var path = Path.GetFullPath(Path.Combine(root, source));
        if (!path.StartsWith(root.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase)) throw new IOException("보관함 밖 경로입니다.");
        for (var parent = new DirectoryInfo(Path.GetDirectoryName(path)!); parent is not null && parent.FullName.StartsWith(root, StringComparison.OrdinalIgnoreCase); parent = parent.Parent)
            if (parent.Exists && parent.Attributes.HasFlag(FileAttributes.ReparsePoint)) throw new IOException("연결된 폴더에는 쓸 수 없습니다.");
        if (File.Exists(path) && File.GetAttributes(path).HasFlag(FileAttributes.ReparsePoint)) throw new IOException("연결된 파일에는 쓸 수 없습니다.");
        return path;
    }

    public static void Ensure(string vault)
    {
        var pending = SafePath(vault, RecoveryPath);
        if (File.Exists(pending))
        {
            Apply(vault, JsonSerializer.Deserialize<List<KnowledgeWrite>>(File.ReadAllText(pending), Json) ?? throw new IOException("이전 구조 전환 기록 오류"));
            File.Delete(pending);
        }
        foreach (var folder in new[] { ".kakaorelay", "01-프로젝트", "02-업무지식", "03-할일", "04-대화기록", "05-변경이력", "검토필요" })
        { var path = SafePath(vault, folder); Directory.CreateDirectory(path); ObsidianKnowledge.CheckDirectory(path); }
        var legacy = Path.Combine(vault, "자동정리");
        if (!Directory.Exists(legacy)) return;
        ObsidianKnowledge.CheckDirectory(legacy);
        foreach (var file in Directory.EnumerateFiles(legacy, "*.md"))
        {
            var source = "자동정리/" + Path.GetFileName(file);
            if (Load(vault).Migrated.Contains(source)) continue;
            var text = Read(vault, source);
            var room = Regex.Match(text, @"\A# (.+) 대화 자동 정리\r?\n");
            if (!room.Success) continue; // Preserve unknown/user-created notes as searchable originals.
            var profile = Regex.Match(text, @"(?m)^계정: (.+)$").Groups[1].Value.Trim();
            var roomId = Regex.Match(text, @"(?m)^대화방 ID: (.+)$").Groups[1].Value.Trim();
            var claims = new List<KnowledgeClaim>();
            foreach (Match match in Regex.Matches(text, @"(?ms)^## ([^\r\n]+)\r?\n\s*(.*?)\r?\n> 종류: (knowledge|decision|task) · 대화에서 확인된 내용.*?(?=^## |\z)"))
                claims.Add(new(match.Groups[1].Value, match.Groups[2].Value.Trim(), match.Groups[3].Value, "confirmed", []));
            if (claims.Count == 0) continue;
            var plan = Plan(vault, new(profile, roomId, room.Groups[1].Value, Guid.Empty), new(claims), [], DateTimeOffset.UtcNow,
                "migration-" + Id(source), true, source, text);
            File.WriteAllText(pending, JsonSerializer.Serialize(plan.Writes, Json), new UTF8Encoding(false));
            Apply(vault, plan.Writes); File.Delete(pending);
        }
    }
}
