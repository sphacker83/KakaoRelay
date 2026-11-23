namespace KakaoRelay.Core;

// Called on the app dispatcher. Each room owns its cancellation; changing one never restarts others.
public sealed class ReplyRuntime(AiService ai, Func<ApiSendCommand, CancellationToken, Task<TestSendReceipt>> send, Func<List<ConversationTarget>>? scan = null)
{
    private sealed class Entry(LocalRoom room) { public LocalRoom Room = room; public CancellationTokenSource Stop = new(); public Task Task = Task.CompletedTask; public string Status = "시작 준비"; }
    private readonly System.Collections.Concurrent.ConcurrentDictionary<string, Entry> entries = new();
    public event Action? Changed;
    public int Count => entries.Count;
    public List<string> Statuses => entries.Values.Select(e => e.Status).ToList();
    public bool Running(LocalRoom room) => entries.ContainsKey(AiSettings.RoomKey(room.Profile, room.Id));
    public async Task StartAsync(List<LocalRoom> rooms, CancellationToken ct = default)
    {
        var selected = rooms.Where(r => !Running(r)).ToList();
        if (selected.Count == 0) return;
        var settings = ai.Settings.Load();
        if (!System.Numerics.BigInteger.TryParse(settings.SelfId, out var id) || id <= 0) throw new InvalidOperationException("카카오 ID를 먼저 설정하세요.");
        var ready = await AutoReplyReadiness.PrepareAsync(ai.Chats, selected, scan ?? ConversationCatalog.Scan, ct);
        foreach (var room in ready.Selected)
        {
            ct.ThrowIfCancellationRequested();
            var key = AiSettings.RoomKey(room.Profile, room.Id);
            if (entries.ContainsKey(key)) continue;
            var entry = new Entry(room);
            if(!entries.TryAdd(key, entry)) { entry.Stop.Dispose(); continue; }
            entry.Task = Run(key, entry, settings.SelfId);
        }
        Changed?.Invoke();
    }
    private async Task Run(string key, Entry entry, string self)
    {
        var session = new AutoReplySession(ai, c => send(c, entry.Stop.Token));
        session.StatusChanged += s => { entry.Status = s; Changed?.Invoke(); };
        try { await session.RunAsync(entry.Room, self, "", entry.Stop.Token); }
        catch (OperationCanceledException) when (entry.Stop.IsCancellationRequested) { }
        catch (Exception e) { LastError = entry.Room.Title + " · " + e.Message; }
        finally { entries.TryRemove(key, out _); entry.Stop.Dispose(); Changed?.Invoke(); }
    }
    public string LastError { get; private set; } = "";
    public async Task StopAsync(LocalRoom? room = null)
    {
        var stop = entries.Values.Where(e => room is null || (e.Room.Profile == room.Profile && e.Room.Id == room.Id)).ToList();
        foreach (var e in stop) { try {e.Stop.Cancel();} catch(ObjectDisposedException) { } }
        await Task.WhenAll(stop.Select(e => e.Task));
    }
    public Task DrainAsync() => Task.WhenAll(entries.Values.Select(e => e.Task).ToArray());
}
