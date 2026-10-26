using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace KakaoRelay.Core;

public sealed record DelegationJob(string Id, string Profile, string RoomId, string RoomTitle, string MessageId,
    string Request, GrokTask Task, DateTimeOffset CreatedAt)
{
    public string RunId { get; init; } = DelegationJobStore.RunId;
    public string Status { get; set; } = "accepting";
    public string? Result { get; set; }
    public string? Error { get; set; }
    public DateTimeOffset UpdatedAt { get; set; } = DateTimeOffset.UtcNow;
}

public sealed class DelegationJobStore(string? root = null)
{
    public static string RunId { get; } = Guid.NewGuid().ToString("N");
    public static string DefaultRoot => Path.Combine(AppContext.BaseDirectory, "data", "delegations");
    private readonly string folder = root ?? DefaultRoot;
    public bool TryCreate(DelegationJob job)
    {
        Directory.CreateDirectory(folder);
        FileStream file;
        try
        {
            file = new FileStream(Path.Combine(folder, job.Id + ".json"), FileMode.CreateNew, FileAccess.Write, FileShare.None);
        }
        catch (IOException) when (File.Exists(Path.Combine(folder, job.Id + ".json"))) { return false; }
        using (file) JsonSerializer.Serialize(file, job, ReportStore.JsonOptions);
        return true;
    }
    public void Save(DelegationJob job, string status, string? error = null)
    {
        job.Status = status; job.Error = error; job.UpdatedAt = DateTimeOffset.UtcNow;
        var path = Path.Combine(folder, job.Id + ".json");
        File.WriteAllText(path + ".tmp", JsonSerializer.Serialize(job, ReportStore.JsonOptions));
        File.Move(path + ".tmp", path, true);
    }
    public List<DelegationJob> List()
    {
        if (!Directory.Exists(folder)) return [];
        var result = new List<DelegationJob>();
        foreach (var path in Directory.EnumerateFiles(folder, "*.json"))
        {
            try
            {
                var job = JsonSerializer.Deserialize<DelegationJob>(File.ReadAllText(path), ReportStore.JsonOptions);
                if (job is null) continue;
                if (job.RunId != RunId && job.Status is "accepting" or "queued" or "running" or "finalizing" or "sending")
                    job.Status = "interrupted";
                result.Add(job);
            }
            catch (Exception error) when (error is IOException or JsonException) { }
        }
        return result.OrderByDescending(j => j.CreatedAt).ToList();
    }
}

// Owned by one room's polling session. Grok work never holds its conversation gate.
public sealed class BackgroundDelegations : IAsyncDisposable
{
    private readonly AiService ai;
    private readonly Func<LocalRoom, LocalMessage, AiResult, string, CancellationToken, Task> send;
    private readonly Action<string> status;
    private readonly CancellationTokenSource stop;
    private readonly DelegationJobStore store;
    private readonly List<Task> workers = [];
    private int pending;
    public int Pending => Volatile.Read(ref pending);
    public BackgroundDelegations(AiService ai, Func<LocalRoom, LocalMessage, AiResult, string, CancellationToken, Task> send,
        Action<string> status, CancellationToken cancellation, string? root = null)
    {
        this.ai = ai; this.send = send; this.status = status;
        stop = CancellationTokenSource.CreateLinkedTokenSource(cancellation); store = new(root);
    }
    public async Task StartAsync(LocalRoom room, LocalMessage message, AiResult plan)
    {
        stop.Token.ThrowIfCancellationRequested();
        var task = plan.Delegation ?? throw new ArgumentException("위임 작업이 없습니다.");
        var id = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes($"{room.Profile}:{room.Id}:{message.Id}")))[..40];
        var job = new DelegationJob(id, room.Profile, room.Id, room.Title, message.Id, message.Text, task, DateTimeOffset.UtcNow);
        if (!store.TryCreate(job)) return; // Never replay a previously admitted job after restart.
        try
        {
            await send(room, message, plan, "", stop.Token);
            stop.Token.ThrowIfCancellationRequested();
            store.Save(job, "queued");
        }
        catch (Exception error)
        {
            store.Save(job, "uncertain", error.GetType().Name);
            throw;
        }
        workers.RemoveAll(t => t.IsCompleted);
        Interlocked.Increment(ref pending);
        workers.Add(Task.Run(() => RunAsync(job, room, message, plan)));
    }
    private async Task RunAsync(DelegationJob job, LocalRoom room, LocalMessage message, AiResult plan)
    {
        var sending = false;
        try
        {
            store.Save(job, "running"); status($"Grok 작업 중 · {job.Id[..8]}");
            var raw = await ai.RunGrokTaskAsync(job.Task, stop.Token);
            job.Result = raw; store.Save(job, "finalizing");
            var final = await ai.GenerateAsync(new(room.Profile, room.Id, "reply", "완료된 백그라운드 작업의 결과를 원래 요청에 맞게 전달하라.")
                { TargetMessageId = message.Id }, stop.Token, deferImages: true,
                completion: new(job.Id, message.Text, job.Task, raw));
            if (final.PersonaId != plan.PersonaId) throw new InvalidOperationException("작업 중 연결된 페르소나가 변경되었습니다.");
            stop.Token.ThrowIfCancellationRequested();
            store.Save(job, "sending"); sending = true;
            await send(room, message, final with { Text = RequestLabel(message) + final.Text, Attachment = null }, "grok-result", stop.Token);
            store.Save(job, "completed"); status($"Grok 결과 전달 완료 · {job.Id[..8]}");
        }
        catch (OperationCanceledException) when (stop.IsCancellationRequested) { Record(sending ? "uncertain" : "cancelled"); }
        catch (Exception error)
        {
            Record(sending ? "uncertain" : "failed", error.GetType().Name);
            status($"Grok 작업 { (sending ? "발송 확인 필요" : "실패") } · 일반 대화 계속");
            if (!sending && !stop.IsCancellationRequested)
            {
                try
                {
                    await send(room, message, plan with { Text = RequestLabel(message) + "검토 작업을 완료하지 못했어요. 다른 대화는 계속할 수 있어요.",
                        Delegation = null, Attachment = null, ImagePath = null, ImagePrompt = null }, "grok-error", stop.Token);
                }
                catch (Exception failure) { Record(stop.IsCancellationRequested ? "cancelled" : "uncertain", failure.GetType().Name); }
            }
        }
        finally { Interlocked.Decrement(ref pending); }
        void Record(string state, string? error = null)
        {
            try { store.Save(job, state, error); }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
        }
    }
    private static string RequestLabel(LocalMessage message)
    {
        var text = message.Text.Replace('\r', ' ').Replace('\n', ' ');
        if (text.Length > 80) text = text[..80] + "…";
        return $"“{text}” 요청 결과\n";
    }
    public async ValueTask DisposeAsync()
    {
        await stop.CancelAsync();
        await Task.WhenAll(workers);
        stop.Dispose();
    }
}
