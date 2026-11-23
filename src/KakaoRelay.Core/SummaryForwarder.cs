using System.Numerics;
using System.Text;
using System.Text.Json;

namespace KakaoRelay.Core;

public sealed class SummaryForwarder(AiService ai, Func<ApiSendCommand, CancellationToken, Task<TestSendReceipt>> send,
    Func<List<ConversationTarget>> scan, string? stateRoot = null, TimeProvider? clock = null)
{
    private readonly SemaphoreSlim gate = new(1);
    private readonly TimeProvider time = clock ?? TimeProvider.System;
    private readonly string root = stateRoot ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "KakaoRelay", "summary-forwards");
    private static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented = true, Encoder = PromptJson.Encoder };
    public event Action<string>? StatusChanged;
    public sealed class State
    {
        public Dictionary<string, string> Cursors { get; set; } = [];
        public DateTimeOffset LastRun { get; set; }
        public List<SummaryUpdate> History { get; set; } = [];
        public Dictionary<string, string> LatestFacts { get; set; } = [];
        public Pending? Pending { get; set; }
        public Dictionary<string, string> SourceErrors { get; set; } = [];
    }
    public sealed record Pending(string RequestId, string Text, List<SummaryUpdate> Items, Dictionary<string, string> Cursors, bool Dispatching = false);
    private static string Identity(SummaryForwardRule r) => AiConversation.Hash(PromptJson.Serialize(new
    { r.Id, Sources = r.Sources.Select(s => s.Key).Order().ToArray(), Destination = r.Destination!.Key }));
    private static string ContentHash(string text) => AiConversation.Hash(text.Normalize().Trim());
    private static string FactKey(SummaryUpdate item) => AiConversation.Hash(PromptJson.Serialize(new
    { Topic = item.Topic.Normalize().Trim(), Sources = item.Evidence.Select(e => e.Source).Distinct().Order().ToArray() }));
    public async Task TickAsync(CancellationToken ct = default)
    {
        if (!await gate.WaitAsync(0, ct)) return;
        try
        {
            var settings = ai.Settings.Load();
            foreach (var rule in settings.SummaryForwards.Where(r => r.Enabled))
            {
                ct.ThrowIfCancellationRequested();
                try { await Process(rule, settings, ct); }
                catch (OperationCanceledException) when (ct.IsCancellationRequested) { throw; }
                catch (Exception error) { Report(rule, "실패: " + error.Message); }
            }
        }
        finally { gate.Release(); }
    }
    private void Report(SummaryForwardRule rule, string message) => StatusChanged?.Invoke($"{rule.Destination!.Title} · {message}");
    private async Task Process(SummaryForwardRule rule, AiSettings settings, CancellationToken ct)
    {
        Directory.CreateDirectory(root);
        var path = Path.Combine(root, Identity(rule) + ".json");
        var state = File.Exists(path) ? JsonSerializer.Deserialize<State>(await File.ReadAllTextAsync(path, ct), Json)
            ?? throw new IOException("전달 상태 파일 오류") : new State();
        var now = time.GetUtcNow();
        if (state.Pending?.Dispatching == true)
        { Report(rule, "전송 결과 확인 필요 · 재발송 중단 · 발송 이력 확인 후 규칙을 껐다 켜세요."); return; }
        if (state.LastRun != default && now - state.LastRun < TimeSpan.FromMinutes(rule.IntervalMinutes)) return;
        void ReportStatus(string message) => Report(rule, message + (state.SourceErrors.Count == 0 ? "" : " · 읽기 실패: " + string.Join(" / ", rule.Sources.Where(s => state.SourceErrors.ContainsKey(s.Key)).Select(s => s.Title + ": " + state.SourceErrors[s.Key]))));
        if (state.Pending is null)
        {
            await ai.Chats.RoomsAsync(ct);
            var inputs = new List<SummaryInput>();
            var baseline = state.Cursors.Count == 0;
            foreach (var source in rule.Sources)
            {
                try
                {
                    var initialized = state.Cursors.TryGetValue(source.Key, out var cursor);
                    var context = await ai.Chats.ReadAsync(source.Profile, source.Id, initialized ? 500 : 1, ct);
                    var messages = context.Messages.OrderBy(m => BigInteger.Parse(m.Id)).ToList();
                    var latest = messages.LastOrDefault()?.Id ?? "0";
                    if (!initialized) state.Cursors[source.Key] = latest;
                    else
                    {
                        if (BigInteger.Parse(latest) < BigInteger.Parse(cursor!)
                            || messages.Count == 500 && BigInteger.Parse(messages[0].Id) > BigInteger.Parse(cursor!))
                            throw new IOException("처리 위치와 조회 범위가 맞지 않습니다. 주기를 줄이고 규칙을 껐다 켜세요.");
                        inputs.AddRange(messages.Where(m => BigInteger.Parse(m.Id) > BigInteger.Parse(cursor!)).Select(m => new SummaryInput(source.Key, m)));
                    }
                    state.SourceErrors.Remove(source.Key);
                }
                catch (OperationCanceledException) when (ct.IsCancellationRequested) { throw; }
                catch (Exception error)
                {
                    // Keep this room's cursor unchanged; other sources can still progress.
                    state.SourceErrors[source.Key] = error.Message;
                }
            }
            if (baseline)
            {
                state.LastRun = now; Save(path, state);
                ReportStatus("읽을 수 있는 방의 시작 기준 저장 · 이후 새 대화부터 정리"); return;
            }
            // Process a bounded prefix, retaining all remaining messages for subsequent intervals.
            var batch = inputs.OrderBy(i => BigInteger.Parse(i.Message.Id)).ThenBy(i => i.Source).Take(100).ToList();
            var attachmentCount = 0;
            batch = batch.TakeWhile(i => i.Message.Type is not (18 or 2) || ++attachmentCount <= 5).ToList();
            var next = new Dictionary<string, string>(state.Cursors);
            foreach (var group in batch.GroupBy(i => i.Source))
                next[group.Key] = group.MaxBy(i => BigInteger.Parse(i.Message.Id))!.Message.Id;
            var textInputs = batch.Where(i => !i.Message.Deleted && i.Message.Type is 1 or 18 or 2 && !string.IsNullOrWhiteSpace(i.Message.Text)
                && !i.Message.Text.StartsWith("[정리·전달]", StringComparison.Ordinal))
                .Select(i => i with { Message = i.Message with { Text = i.Message.Text[..Math.Min(i.Message.Text.Length, 3000)] } }).ToList();
            for (var i = 0; i < textInputs.Count; i++)
                textInputs[i] = textInputs[i] with { Message = await ai.Chats.ReadAttachmentAsync(textInputs[i].Message, ct) };
            state.LastRun = now; Save(path, state);
            var digest = textInputs.Count == 0 ? new SummaryDigest([]) :
                await ai.Summaries.GenerateAsync(settings, textInputs, state.History.TakeLast(80).ToList(), ct);
            var updates = digest.Items.DistinctBy(i => ContentHash(i.Text))
                .Where(i => state.LatestFacts.GetValueOrDefault(FactKey(i)) != ContentHash(i.Text)).ToList();
            if (updates.Count == 0)
            {
                state.Cursors = next; state.LastRun = now; Save(path, state);
                ReportStatus("추가사항 없음 · 발송 생략"); return;
            }
            var body = new StringBuilder("[정리·전달]\n");
            body.AppendLine(string.Join(", ", rule.Sources.Select(s => s.Title)));
            body.AppendLine(now.ToLocalTime().ToString("MM/dd HH:mm"));
            foreach (var item in updates)
            {
                var sources = rule.Sources.Where(s => item.Evidence.Any(e => e.Source == s.Key)).Select(s => s.Title);
                body.AppendLine($"\n• {item.Topic}\n{item.Text}\n출처: {string.Join(", ", sources)}");
            }
            state.Pending = new("summary-" + Guid.NewGuid().ToString("N"), body.ToString(), updates, next);
            Save(path, state);
        }
        // Resolve by account/room identity again; never route to a renamed or ambiguous window.
        var currentSettings = ai.Settings.Load();
        var currentRule = currentSettings.SummaryForwards.FirstOrDefault(r => r.Enabled && Identity(r) == Identity(rule));
        if (currentRule is null) return;
        var rooms = await ai.Chats.RoomsAsync(ct);
        var destination = rooms.SingleOrDefault(r => r.Profile == rule.Destination!.Profile && r.Id == rule.Destination.Id)
            ?? throw new IOException("수신방을 로컬 목록에서 찾지 못했습니다.");
        var windows = await Task.Run(scan, ct);
        if (!AutoReplyReadiness.OpenRooms(rooms, windows).Any(r => r.Profile == destination.Profile && r.Id == destination.Id))
        {
            state.LastRun = now; Save(path, state);
            Report(rule, "수신방을 열어주세요 · 요약 보관, 다음 주기에 재확인"); return;
        }
        ct.ThrowIfCancellationRequested();
        state.Pending = state.Pending! with { Dispatching = true };
        Save(path, state); // A crash/uncertain send must never replay automatically.
        var pending = state.Pending;
        var receipt = await send(new(pending.RequestId, destination.Title, pending.Text), ct);
        if (!receipt.EnterPosted || receipt.InputCleared != true)
            throw new IOException("전송 결과 확인 필요 · 자동 재발송 중단");
        state.Cursors = pending.Cursors;
        state.History.AddRange(pending.Items);
        foreach (var item in pending.Items) state.LatestFacts[FactKey(item)] = ContentHash(item.Text);
        state.Pending = null; state.LastRun = time.GetUtcNow();
        Save(path, state);
        ReportStatus($"추가사항 {pending.Items.Count}건 전달 완료");
    }
    private static void Save(string path, State state)
    {
        File.WriteAllText(path + ".tmp", JsonSerializer.Serialize(state, Json), new UTF8Encoding(false));
        File.Move(path + ".tmp", path, true);
    }
}
