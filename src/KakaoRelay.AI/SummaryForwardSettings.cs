namespace KakaoRelay.Core;

public sealed record SummaryRoom(string Profile, string Id, string Title)
{
    public string Key => AiSettings.RoomKey(Profile, Id);
}
public sealed class SummaryForwardRule
{
    public string Id { get; set; } = Guid.NewGuid().ToString("N");
    public bool Enabled { get; set; }
    public int IntervalMinutes { get; set; } = 30;
    public List<SummaryRoom> Sources { get; set; } = [];
    public SummaryRoom? Destination { get; set; }
    public string Label => $"{string.Join(", ", Sources.Select(r => r.Title))} → {Destination?.Title} · {IntervalMinutes}분{(Enabled ? "" : " · 꺼짐")}";
    public void Validate()
    {
        if (!Guid.TryParseExact(Id, "N", out _) || IntervalMinutes is < 1 or > 1440 || Sources is null || Destination is null
            || Sources.Count == 0 || Sources.Any(r => !Valid(r)) || !Valid(Destination)
            || Sources.Select(r => r.Key).Distinct().Count() != Sources.Count || Sources.Any(r => r.Key == Destination.Key))
            throw new ArgumentException("정리할 방과 다른 수신방, 주기(1~1440분)를 지정하세요.");
    }
    private static bool Valid(SummaryRoom? r) => r is not null && !string.IsNullOrWhiteSpace(r.Profile)
        && !string.IsNullOrWhiteSpace(r.Id) && !string.IsNullOrWhiteSpace(r.Title)
        && r.Profile.Length <= 200 && r.Id.Length <= 200 && r.Title.Length <= 500;
}
