// Rust 이식 호환 기준 출력. 실행: dotnet run --project tools/golden -- <출력 JSON 경로>
// 각 항목은 {name, input(.NET 직렬화 문자열), hash(SHA256 대문자 hex)}이다. Core의 익명 객체는 원본 식을 그대로 복제했다.
using System.Text.Json;
using System.Text.Encodings.Web;
using System.Text.Unicode;
using KakaoRelay.Core;

var output = args.Length > 0 ? args[0] : "golden.json";
var cases = new List<object>();
void Add(string name, string input) => cases.Add(new { name, input, hash = AiConversation.Hash(input) });

var tricky = "한글 \"따옴표\" <tag> & ' + \\ 줄\n바꿈\t탭 😀 é \u0001 \u2028";
var reportOptions = new JsonSerializerOptions { WriteIndented = true, PropertyNamingPolicy = JsonNamingPolicy.CamelCase, Encoder = PromptJson.Encoder };
var webOptions = new JsonSerializerOptions(JsonSerializerDefaults.Web) { WriteIndented = true, Encoder = PromptJson.Encoder };

Add("roomKey.ascii", AiSettings.RoomKey("ABCDEF0123", "18273645"));
Add("roomKey.tricky", AiSettings.RoomKey(tricky, "1"));
Add("promptJson.tricky", PromptJson.Serialize(new { Text = tricky, List = new[] { "가", "b" } }));
Add("default.tricky", JsonSerializer.Serialize(new { Text = tricky }));

var times = new[] { new DateTimeOffset(2026, 9, 28, 13, 23, 0, TimeSpan.FromHours(9)), new DateTimeOffset(2026, 9, 28, 13, 23, 0, 120, TimeSpan.FromHours(9)), new DateTimeOffset(2026, 1, 2, 3, 4, 5, TimeSpan.Zero).AddTicks(1234567 - 1234567 % 1 + 1) };
for (var i = 0; i < times.Length; i++)
    Add($"messageFingerprint.{i}", JsonSerializer.Serialize(new LocalMessage("9001", "77", "홍길동", times[i], 1, tricky, false) { AttachmentMetadata = "{\"x\":1}" }));

var command = new AiCommand("ABCDEF0123", "18273645", "reply", "질문");
var provider = new AiProviderSettings { Id = "codex", Executable = "", Model = "gpt-6-luna", Effort = "low" };
var persona = new BotPersona();
Add("conversationFolder.default", JsonSerializer.Serialize(new { command.Profile, command.RoomId, provider, persona }));
var persona2 = new BotPersona { Name = "봇", Role = tricky, Tone = "반말", Style = "짧게", Instructions = "" };
var provider2 = new AiProviderSettings { Id = "claude", Enabled = false, Executable = "/opt/homebrew/bin/claude", Model = "claude-opus-5-5[1m]", Effort = "default" };
Add("conversationFolder.custom", JsonSerializer.Serialize(new { command.Profile, command.RoomId, provider = provider2, persona = persona2 }));

// Core/TestSend.cs:22
Add("testSendFingerprint", JsonSerializer.Serialize(new { Recipient = "나와의 채팅", Message = tricky, ExpectedProcessId = 1234, ExpectedWindowHandle = "0x1A2B", VerifiedEmptyPlaceholder = true }));
// Core/RelayApi.cs:34-36
Add("apiFingerprint.text", JsonSerializer.Serialize(new { Recipient = "방", Message = tricky }));
Add("apiFingerprint.image", JsonSerializer.Serialize(new { Recipient = "방", ImagePath = "/Users/x/그림 1.png" }));
Add("apiFingerprint.attachment", JsonSerializer.Serialize(new { Recipient = "방", Message = "", Attachment = new PersonaAttachment("default", "astra", "0123456789abcdef0123456789abcdef") }));
// Core/SummaryForwarder.cs:25-29
Add("summaryIdentity", PromptJson.Serialize(new { Id = "0123456789abcdef0123456789abcdef", Sources = new[] { AiSettings.RoomKey("P", "2"), AiSettings.RoomKey("P", "10") }.Order().ToArray(), Destination = AiSettings.RoomKey("P", "3") }));
Add("summaryFactKey", PromptJson.Serialize(new { Topic = "출시 일정".Normalize().Trim(), Sources = new[] { "방B", "방A", "방B" }.Distinct().Order().ToArray() }));
// AI/AutoKnowledgeService.cs:56
Add("autoKnowledgeCheckpoint", PromptJson.Serialize(new { Profile = "P", RoomId = "2", Generation = "8f0d7c1e-6b1a-4c55-9b4e-0d7d0c3f2a11", vault = "/Users/x/보관함", SelfId = "12345" }));
// Core/DailyReplyHistory.cs Entry(DateOnly Day, string FirstReply)
Add("dailyReplyEntry", JsonSerializer.Serialize(new { Day = new DateOnly(2026, 11, 23), FirstReply = "bot-ABC" }));
// AI/AiConversation.cs state.json
var state = new AiConversation { SessionId = "s-1", KnowledgeRevision = null, InstructionsDate = new DateOnly(2026, 11, 23), InstructionsRevision = "R" };
state.Seen.Add("H1");
Add("conversationState", JsonSerializer.Serialize(state));
Add("reportOptions.time", JsonSerializer.Serialize(new { StartedAt = times[1], FinishedAt = (DateTimeOffset?)null, InputCleared = (bool?)true, Detail = tricky }, reportOptions));

// ai-settings.json: 기본값 저장 결과와 이전 단일 persona 형식의 승격
var root = Path.Combine(Path.GetTempPath(), "kakaorelay-golden-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(root);
var settingsPath = Path.Combine(root, "ai-settings.json");
new AiSettingsStore(settingsPath).Save(new AiSettings());
var settingsDefault = File.ReadAllText(settingsPath);
File.WriteAllText(settingsPath, "{\"persona\":{\"name\":\"예전봇\",\"role\":\"r\",\"tone\":\"t\",\"style\":\"s\",\"instructions\":\"i\"},\"provider\":\"grok\",\"pollSeconds\":7}");
var legacy = new AiSettingsStore(settingsPath).Load();
new AiSettingsStore(settingsPath).Save(legacy);
var settingsLegacy = File.ReadAllText(settingsPath);
Directory.Delete(root, true);

File.WriteAllText(output, JsonSerializer.Serialize(new { generatedBy = ".NET " + Environment.Version, cases, settingsDefault, settingsLegacyResaved = settingsLegacy }, webOptions));
Console.WriteLine($"wrote {cases.Count} cases to {output}");
