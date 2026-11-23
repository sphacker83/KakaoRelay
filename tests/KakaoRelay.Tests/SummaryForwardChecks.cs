using System.Text.Json;
using KakaoRelay.Core;

internal static class SummaryForwardChecks
{
    private sealed class Reader : IChatReader
    {
        public List<LocalRoom> Rooms = [new("test", "1", "원본", true), new("test", "2", "수신", true)];
        public List<LocalMessage> Messages = [];
        public HashSet<string> FailedRooms = [];
        public Task<LocalMessage> ReadAttachmentAsync(LocalMessage message, CancellationToken cancellation = default) =>
            Task.FromResult(message.Type == 18 ? message with { Text = message.Text + "\n첨부 결정: 금요일 제출" } : message);
        public Task<List<LocalRoom>> RoomsAsync(CancellationToken cancellation = default) => Task.FromResult(Rooms.ToList());
        public Task<ChatContext> ReadAsync(string profile, string roomId, int limit, CancellationToken cancellation = default) =>
            FailedRooms.Contains(roomId) ? Task.FromException<ChatContext>(new IOException("test room unavailable")) : Task.FromResult(new ChatContext(Rooms.Single(r => r.Id == roomId), Messages.TakeLast(limit).ToList(), DateTimeOffset.UtcNow));
    }
    private sealed class Runner : IAiRunner
    {
        public int Calls;
        public Action? BeforeResult;
        public SummaryDigest Result = new([]);
        public Task<string> RunAsync(AiProviderSettings provider, string prompt, int timeoutSeconds, CancellationToken cancellation)
        { Calls++; BeforeResult?.Invoke(); return Task.FromResult(JsonSerializer.Serialize(Result, new JsonSerializerOptions(JsonSerializerDefaults.Web))); }
    }
    private sealed class Clock : TimeProvider
    {
        public DateTimeOffset Now = new(2026, 10, 20, 1, 0, 0, TimeSpan.Zero);
        public override DateTimeOffset GetUtcNow() => Now;
        public void Advance() => Now = Now.AddMinutes(1);
    }
    public static async Task RunAsync(string root, Action<bool, string> check)
    {
        var reader = new Reader(); var runner = new Runner(); var clock = new Clock();
        var store = new AiSettingsStore(Path.Combine(root, "summary-settings.json"));
        var settings = store.Load();
        var source = new SummaryRoom("test", "1", "원본");
        var rule = new SummaryForwardRule { Enabled = true, IntervalMinutes = 1, Sources = [source], Destination = new("test", "2", "수신") };
        settings.SummaryForwards = [rule]; settings.SelectedBotRooms = [source.Key];
        foreach (var p in settings.Providers) p.Enabled = p.Id == "codex";
        store.Save(settings);
        var ai = new AiService(reader, store, runner);
        var sent = new List<ApiSendCommand>(); bool uncertain = false, open = true;
        var state = Path.Combine(root, "summary-state");
        SummaryForwarder Forwarder() => new(ai, (command, ct) =>
        {
            sent.Add(command);
            return Task.FromResult(new TestSendReceipt { RequestId = command.RequestId, EnterPosted = true, InputCleared = !uncertain });
        }, () => open ? [new("수신", 1, "0x10", false)] : [], state, clock);
        void Add(string id, string text) => reader.Messages.Add(new(id, "human", "작성자", clock.Now, 1, text, false));
        void Result(string id, string text) => runner.Result = new([new("일정", text, [new(source.Key, id, text)])]);
        var forwarder = Forwarder();
        Add("1", "기존 대화");
        await forwarder.TickAsync();
        check(sent.Count == 0 && runner.Calls == 0, "Summary forwarding baselines existing messages without generating or sending");
        Add("2", "금요일 확정"); Result("2", "금요일 확정");
        await forwarder.TickAsync();
        check(sent.Count == 0 && runner.Calls == 0, "Summary forwarding respects the configured interval");
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == 1 && sent[0].Recipient == "수신" && sent[0].Message.Contains("금요일 확정") && sent[0].Message.Contains("출처: 원본"),
            "New evidenced updates are summarized and forwarded only to the configured recipient");
        forwarder = Forwarder(); clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == 1 && runner.Calls == 1, "Restart retains the cursor and no new messages cause no model calls or sends");
        Add("3", "금요일 확정"); Result("3", "금요일 확정");
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == 1, "Repeated current facts do not produce duplicate notifications");
        Add("4", "안녕하세요"); runner.Result = new([]);
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == 1, "New chatter without substantive additions advances the cursor without sending");
        Add("5", "월요일 변경"); Result("5", "월요일 변경");
        open = false; clock.Advance(); await forwarder.TickAsync(); var calls = runner.Calls;
        check(sent.Count == 1, "Closed recipient keeps the prepared summary without dispatching");
        forwarder = Forwarder(); open = true; clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == 2 && runner.Calls == calls && sent[1].Message.Contains("월요일 변경"), "A retained summary resumes after restart when its recipient opens without regenerating");
        Add("6", "금요일 확정"); Result("6", "금요일 확정");
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == 3, "A genuine change back to an earlier state can be reported again");
        Add("7", "화요일 변경"); Result("7", "화요일 변경"); uncertain = true;
        clock.Advance(); await forwarder.TickAsync();
        var attempts = sent.Count; forwarder = Forwarder(); clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == attempts, "Uncertain dispatch survives restart and is never automatically resent");
        rule.Id = Guid.NewGuid().ToString("N"); store.Save(settings); uncertain = false;
        await forwarder.TickAsync();
        check(sent.Count == attempts, "Re-enabled fresh rule baselines instead of replaying an uncertain past send");
        Add("8", "수요일 확정"); Result("8", "조작된 인용");
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == attempts, "Unverifiable model evidence cannot cause a send");
        Result("8", "수요일 확정"); clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == attempts + 1, "Failed generation preserves the cursor so the new facts can be retried");
        settings.SelectedBotRooms.Clear(); store.Save(settings);
        Add("9", "목요일 변경"); Result("9", "목요일 변경");
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == attempts + 2, "Summary monitoring continues with no automatic-reply rooms selected");
        Add("10", "토요일 변경"); Result("10", "토요일 변경");
        runner.BeforeResult = () => { rule.Enabled = false; store.Save(settings); };
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == attempts + 2, "Disabling a rule while its summary is being generated prevents dispatch");
        runner.BeforeResult = null;
        rule.Enabled = true; rule.Id = Guid.NewGuid().ToString("N"); store.Save(settings);
        await forwarder.TickAsync();
        reader.Messages.Clear();
        foreach (var n in Enumerable.Range(20, 500)) Add(n.ToString(), "대화");
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == attempts + 2, "A source exceeding the 500-message read window fails without silently advancing or sending");
        var invalid = new SummaryForwardRule { Sources = [source], Destination = source };
        reader.Messages.Clear(); rule.Id = Guid.NewGuid().ToString("N"); store.Save(settings);
        await forwarder.TickAsync();
        reader.Messages.Add(new("600", "human", "작성자", clock.Now, 18, "회의록.pdf", false));
        Result("600", "첨부 결정: 금요일 제출");
        var beforeFile = sent.Count;
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == beforeFile + 1 && sent[^1].Message.Contains("금요일 제출"), "Summary monitoring extracts file contents and validates quotes against extracted text");
        // A failing source must not block a healthy source or lose its saved cursor.
        reader.Rooms.Add(new("test", "3", "오류방", true));
        rule.Sources.Add(new("test", "3", "오류방"));
        rule.Id = Guid.NewGuid().ToString("N"); store.Save(settings);
        await forwarder.TickAsync();
        reader.FailedRooms.Add("3");
        Add("601", "일요일 변경"); Result("601", "일요일 변경");
        var beforePartial = sent.Count;
        string status = ""; forwarder.StatusChanged += value => status = value;
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == beforePartial + 1 && status.Contains("오류방"), "A failed room is named while healthy room updates are delivered");
        var partial = Directory.GetFiles(state, "*.json").Select(f => JsonSerializer.Deserialize<SummaryForwarder.State>(File.ReadAllText(f), new JsonSerializerOptions(JsonSerializerDefaults.Web))!).Single(s => s.SourceErrors.ContainsKey(AiSettings.RoomKey("test", "3")));
        check(partial.Cursors[source.Key] == "601" && partial.Cursors[AiSettings.RoomKey("test", "3")] == "600", "Failed room cursor stays unchanged while healthy room advances");
        reader.FailedRooms.Clear();
        runner.Result = new([new("복구", "일요일 변경", [new(AiSettings.RoomKey("test", "3"), "601", "일요일 변경")])]);
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == beforePartial + 2, "Recovered room resumes unread messages from its retained cursor");
        rule.Id = Guid.NewGuid().ToString("N"); store.Save(settings);
        reader.FailedRooms.Add("3"); await forwarder.TickAsync();
        Add("602", "다음 주 확정"); Result("602", "다음 주 확정");
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == beforePartial + 3, "A source unavailable at initial baseline does not block healthy source updates");
        reader.FailedRooms.Clear(); runner.Result = new([]);
        clock.Advance(); await forwarder.TickAsync();
        check(sent.Count == beforePartial + 3, "First successful read of an unavailable source baselines without replaying history");
        try { invalid.Validate(); check(false, "Self forwarding"); }
        catch (ArgumentException) { check(true, "A forwarding rule cannot send summaries back to its own source room"); }
    }
}
