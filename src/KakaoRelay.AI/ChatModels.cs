namespace KakaoRelay.Core;

public sealed class ApiFailure(int statusCode, string code, string detail) : Exception(detail)
{
    public int StatusCode { get; } = statusCode;
    public string Code { get; } = code;
}
public sealed record LocalRoom(string Profile, string Id, string Title, bool Readable)
{
    public string? DirectMemberId { get; init; }
    public string Display => $"{Title} · {(Readable ? "읽기 가능" : "카카오톡에서 방을 열어주세요")}";
}
public sealed record LocalMessage(string Id, string AuthorId, string Author, DateTimeOffset Time, int Type, string Text, bool Deleted)
{
    [System.Text.Json.Serialization.JsonIgnore]
    public string? AttachmentMetadata { get; init; }
    [System.Text.Json.Serialization.JsonIgnore]
    public AgentAttachment? Attachment { get; init; }
}
public sealed record ChatContext(LocalRoom Room, List<LocalMessage> Messages, DateTimeOffset ReadAt);
public interface IChatReader
{
    async Task<bool> IsDirectPeerAsync(LocalRoom room, string peer, CancellationToken cancellation = default) =>
        (await RoomsAsync(cancellation)).Any(r=>r.Profile==room.Profile&&r.Id==room.Id&&r.DirectMemberId==peer);
    Task<LocalMessage> ReadAttachmentAsync(LocalMessage message, CancellationToken cancellation = default) => Task.FromResult(message);
    Task<List<LocalRoom>> RoomsAsync(CancellationToken cancellation = default);
    Task<ChatContext> ReadAsync(string profile, string roomId, int limit, CancellationToken cancellation = default);
}
