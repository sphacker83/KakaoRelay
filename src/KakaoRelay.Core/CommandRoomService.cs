using System.Numerics;
using System.Text.Json;

namespace KakaoRelay.Core;

public sealed class CommandRoomSettings
{
    public bool Enabled { get; set; }
    public bool NaturalLanguage { get; set; }
    public LocalRoom? Room { get; set; }
    public string OperatorId { get; set; } = "";
    public Guid Generation { get; set; } = Guid.NewGuid();
    public bool Reply { get; set; } = true;
    public bool Knowledge { get; set; } = true;
    public bool Forward { get; set; }
    public bool Provider { get; set; }
    public bool Owns(LocalRoom room) => Enabled && Room?.Profile == room.Profile && Room.Id == room.Id;
    public void Validate()
    {
        if (Generation == Guid.Empty || (Enabled && (Room is null || !BigInteger.TryParse(OperatorId, out var id) || id<=0 || Room.DirectMemberId != OperatorId)))
            throw new ArgumentException("일반 1:1 방과 해당 상대방을 확인하세요.");
    }
}
public sealed class CommandRoomStore(string? folder = null)
{
    public static string DefaultRoot => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "KakaoRelay", "command-room");
    public string Root { get; } = folder ?? DefaultRoot;
    public static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented=true, Encoder=PromptJson.Encoder };
    public CommandRoomSettings Load() => File.Exists(Path.Combine(Root,"settings.json")) ? JsonSerializer.Deserialize<CommandRoomSettings>(File.ReadAllText(Path.Combine(Root,"settings.json")),Json)! : new();
    public void Save(CommandRoomSettings value) { value.Validate(); Write("settings.json",value); }
    public void Write<T>(string name,T value)
    {
        Directory.CreateDirectory(Root); var path=Path.Combine(Root,name);
        File.WriteAllText(path+".tmp",JsonSerializer.Serialize(value,Json)); File.Move(path+".tmp",path,true);
    }
    public T? Read<T>(string name) => File.Exists(Path.Combine(Root,name)) ? JsonSerializer.Deserialize<T>(File.ReadAllText(Path.Combine(Root,name)),Json) : default;
}
public sealed record CommandPlan(RelayCommand Command, AiSettings Next, LocalRoom? Room, string Description, string Revision);
public static class CommandPolicy
{
    public static string Revision(AiSettings settings) => AiConversation.Hash(JsonSerializer.Serialize(settings));
    public static string Snapshot(AiSettings s) => JsonSerializer.Serialize(new { s.SelectedBotRooms, s.RoomReplyModes, s.RoomPersonas, s.AutoKnowledge, s.SummaryForwards, s.Provider },CommandRoomStore.Json);
    public static CommandPlan Prepare(RelayCommand c, AiSettings current, IReadOnlyList<LocalRoom> rooms, CommandRoomSettings policy)
    {
        if (c.Action is null || c.Target is null || c.Value is null || c.Target.Length>500 || c.Value.Length>500) throw new ArgumentException("잘못된 명령입니다.");
        var next=current.Copy(); LocalRoom? target=null; var description="";
        LocalRoom Resolve()
        {
            var matches=rooms.Where(r=>r.Title==c.Target || r.Id==c.Target || AiSettings.RoomKey(r.Profile,r.Id)==c.Target).ToList();
            if(matches.Count!=1)throw new ArgumentException("방을 하나로 특정할 수 없습니다. /상태에서 정확한 이름 또는 ID를 확인하세요.");
            return matches[0];
        }
        bool Toggle() => c.Value switch { "켜기"=>true,"끄기"=>false,_=>throw new ArgumentException("켜기 또는 끄기를 지정하세요.") };
        int Minutes(int minimum) => int.TryParse(c.Value,out var n)&&n>=minimum&&n<=1440?n:throw new ArgumentException($"주기는 {minimum}~1440분입니다.");
        void Permit(bool allowed) { if(!allowed)throw new InvalidOperationException("앱에서 이 제어 기능을 허용하지 않았습니다."); }
        switch(c.Action)
        {
            case "reply":
                Permit(policy.Reply); var on=Toggle();
                if(c.Target=="전체"&&!on) {description="자동답장 전체 중지"; break;}
                target=Resolve();
                if(policy.Owns(target))throw new ArgumentException("제어방은 일반 자동답장과 분리됩니다.");
                var key=AiSettings.RoomKey(target.Profile,target.Id);
                if(on) { if(!next.SelectedBotRooms.Contains(key))next.SelectedBotRooms.Add(key); } else next.SelectedBotRooms.Remove(key);
                description=$"{target.Title} 자동답장 {c.Value}"; break;
            case "mode":
                Permit(policy.Reply); target=Resolve();
                next.RoomReplyModes[AiSettings.RoomKey(target.Profile,target.Id)]=c.Value switch {"즉시"=>"immediate","호출"=>"trigger","맥락"=>"context",_=>throw new ArgumentException("답변 방식은 즉시/호출/맥락입니다.")};
                description=$"{target.Title} 답변 방식 → {c.Value}";break;
            case "persona":
                Permit(policy.Reply); target=Resolve(); var people=next.Personas.Where(p=>p.Id==c.Value||p.Persona.Name==c.Value).ToList();
                if(people.Count!=1)throw new ArgumentException("봇이름이 없거나 중복됩니다.");
                next.RoomPersonas[AiSettings.RoomKey(target.Profile,target.Id)]=people[0].Id;
                description=$"{target.Title} 봇 → {people[0].Persona.Name}";break;
            case "knowledge":
                Permit(policy.Knowledge); target=Resolve(); var add=Toggle();
                if(add&&!Directory.Exists(next.ResolvePersona(target.Profile,target.Id).Knowledge.VaultPath))throw new ArgumentException("앱에서 이 방의 봇에 보관함을 먼저 연결하세요.");
                var config=next.AutoKnowledge;
                if(add) {if(!config.Rooms.Any(r=>r.Profile==target.Profile&&r.RoomId==target.Id))config.Rooms.Add(new(target.Profile,target.Id,target.Title,Guid.NewGuid())); config.Enabled=true;}
                else {config.Rooms.RemoveAll(r=>r.Profile==target.Profile&&r.RoomId==target.Id); if(config.Rooms.Count==0)config.Enabled=false;}
                description=$"{target.Title} 지식 자동 축적 {c.Value}"+(add?" · 새로 추가하면 지금 이후 대화부터":"");break;
            case "knowledge-interval":
                Permit(policy.Knowledge); next.AutoKnowledge.IntervalMinutes=Minutes(5); description=$"지식 축적 주기 → {c.Value}분";break;
            case "knowledge-run":
                Permit(policy.Knowledge); if(!next.AutoKnowledge.Enabled)throw new ArgumentException("지식 자동 축적이 꺼져 있습니다.");description="등록된 방의 지식 정리 지금 실행";break;
            case "forward": case "forward-interval":
                Permit(policy.Forward); var rule=next.SummaryForwards.SingleOrDefault(r=>r.Id==c.Target)??throw new ArgumentException("/상태에 나온 정확한 정리전달 규칙 ID를 지정하세요.");
                if(c.Action=="forward") {var enable=Toggle(); if(enable&&!rule.Enabled)throw new ArgumentException("꺼진 전달 경로 재활성화는 앱에서 수신방을 확인한 뒤 적용하세요.");rule.Enabled=enable;}
                else rule.IntervalMinutes=Minutes(1);
                description=rule.Label;break;
            case "provider":
                Permit(policy.Provider); if(c.Value!="auto"&&!next.Providers.Any(p=>p.Id==c.Value&&p.Enabled))throw new ArgumentException("앱에 등록되고 활성화된 프로바이더만 선택할 수 있습니다.");
                next.Provider=c.Value;description=$"프로바이더 → {c.Value}";break;
            default: throw new ArgumentException("지원하지 않거나 관리 의도가 불명확한 명령입니다. /도움말");
        }
        next.Validate(); return new(c,next,target,description,Revision(current));
    }
}
public sealed class CommandRoomService(AiService ai, CommandRoomStore store,
    Func<AiSettings> settings, Func<CommandPlan,CancellationToken,Task<string>> execute,
    Func<string> runtimeStatus, Func<ApiSendCommand,CancellationToken,Task<TestSendReceipt>> send)
{
    public sealed record Audit(string MessageId, string Action, string Result, DateTimeOffset At, string? Before=null, string? After=null);
    public sealed record Confirmation(string Code, RelayCommand Command, string Revision, DateTimeOffset Expires);
    public sealed class Progress { public string Cursor {get;set;}="0"; public Confirmation? Pending {get;set;} public List<Audit> History {get;set;}=[]; }
    private readonly SemaphoreSlim gate=new(1);
    private AutoReplySession? conversation;
    private BackgroundDelegations? jobs;
    private Guid conversationGeneration;
    private readonly CancellationTokenSource lifetime = new();
    public void Stop() => lifetime.Cancel();
    public async Task DrainAsync()
    {
        await gate.WaitAsync();
        try { if (jobs is not null) { await jobs.DisposeAsync(); jobs = null; } }
        finally { gate.Release(); }
    }
    private async Task PrepareConversation(CommandRoomSettings config)
    {
        if (conversationGeneration == config.Generation && jobs is not null) return;
        if (jobs is not null) await jobs.DisposeAsync();
        var room = config.Room!;
        async Task<TestSendReceipt> SendConversation(ApiSendCommand command)
        {
            lifetime.Token.ThrowIfCancellationRequested();
            var current = store.Load();
            if (!current.Enabled || current.Generation != config.Generation)
                throw new OperationCanceledException("제어방 설정 변경으로 답변을 중단했습니다.");
            var rooms = await ai.Chats.RoomsAsync(lifetime.Token);
            var fresh = rooms.SingleOrDefault(r => r.Profile == room.Profile && r.Id == room.Id);
            if (fresh is null || !await ai.Chats.IsDirectPeerAsync(fresh, config.OperatorId, lifetime.Token)
                || rooms.Count(r => r.Title == fresh.Title) != 1)
                throw new InvalidOperationException("대화 수신방 검증 실패");
            return await send(command with { Recipient = fresh.Title }, lifetime.Token);
        }
        conversation = new(ai, SendConversation);
        conversation.StatusChanged += Report;
        jobs = new(ai, (r,m,result,phase,ct) => conversation.SendReplyAsync(r,m,result,ct,phase), Report, lifetime.Token);
        conversationGeneration = config.Generation;
    }
    public event Action<string>? StatusChanged;
    public bool Busy {get;private set;}
    public string Status {get;private set;}="제어방 꺼짐";
    private void Report(string s) {Status=s;StatusChanged?.Invoke(s);}
    public List<Audit> History() {var p=store.Load();return store.Read<Progress>(p.Generation+".json")?.History??[];}
    public async Task TickAsync(CancellationToken ct=default)
    {
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(ct, lifetime.Token);
        ct = linked.Token;
        if(!await gate.WaitAsync(0,ct))return;
        Busy=true;
        try
        {
            var config=store.Load(); config.Validate(); if(!config.Enabled)return;
            var room=config.Room!; ChatContext context;
            try {context=await ai.Chats.ReadAsync(room.Profile,room.Id,500,ct);}
            catch(ApiFailure e) when(e.Code=="room_not_loaded") {await ai.Chats.RoomsAsync(ct);context=await ai.Chats.ReadAsync(room.Profile,room.Id,500,ct);}
            if(!await ai.Chats.IsDirectPeerAsync(room,config.OperatorId,ct) || config.OperatorId==settings().SelfId)throw new InvalidOperationException("제어방의 1:1 상대 검증 실패. 앱에서 다시 지정하세요.");
            var name=config.Generation+".json"; var progress=store.Read<Progress>(name);
            var ordered=context.Messages.OrderBy(m=>BigInteger.Parse(m.Id)).ToList();
            if(progress is null) {store.Write(name,new Progress{Cursor=ordered.LastOrDefault()?.Id??"0"}); Report("제어방 시작 · 이후 새 명령부터 처리");return;}
            if(ordered.Count>0&&BigInteger.Parse(ordered[^1].Id)<BigInteger.Parse(progress.Cursor))throw new InvalidOperationException("제어방 기록이 이전보다 줄었습니다. 앱에서 다시 적용하세요.");
            if(ordered.Count==500&&BigInteger.Parse(ordered[0].Id)>BigInteger.Parse(progress.Cursor))throw new InvalidOperationException("제어방 새 메시지가 조회 범위를 초과했습니다. 앱에서 다시 적용하세요.");
            foreach(var message in ordered.Where(m=>BigInteger.Parse(m.Id)>BigInteger.Parse(progress.Cursor)).ToList())
            {
                ct.ThrowIfCancellationRequested();
                if(store.Load().Generation!=config.Generation||!store.Load().Enabled)return;
                progress.Cursor=message.Id;
                if(message.Deleted||message.Type is not (1 or 26 or 2 or 18)||message.AuthorId!=config.OperatorId) {store.Write(name,progress);continue;}
                // Consume before interpretation/execution: a crash must never replay a mutation.
                progress.History.Add(new(message.Id,"received","처리 중 · 중단 시 자동 재실행 안 함",DateTimeOffset.UtcNow));
                progress.History=progress.History.TakeLast(100).ToList(); store.Write(name,progress);
                string reply; RelayCommand? command=null; string? before=null,after=null;
                var conversationHandled = false;
                async Task<string> ChatReply()
                {
                    command = new("chat");
                    conversationHandled = true;
                    await PrepareConversation(config);
                    var result = await conversation!.ReplyToAsync(room, message, jobs!, ct);
                    return result.Text;
                }
                try
                {
                    var slash=message.Type is 1 or 26 && message.Text.TrimStart().StartsWith('/');
                    if(message.Type is 2 or 18 || (!slash&&!config.NaturalLanguage)) {reply=await ChatReply();}
                    else
                    {
                        command=slash?RelayCommandParser.Parse(message.Text):await ai.Commands.InterpretAsync(message.Text,settings(),ct);
                        if(!slash && command.Action is "confirm" or "cancel")throw new ArgumentException("확인은 /확인 번호, 취소는 /취소로 보내세요.");
                        if(store.Load().Generation!=config.Generation||!store.Load().Enabled)return;
                        if(!slash&&command.Action=="none")reply=await ChatReply();
                        else if(command.Action=="status")reply=runtimeStatus()+"\n지식 축적: "+(settings().AutoKnowledge.Enabled?"켜짐":"꺼짐")+" · "+settings().AutoKnowledge.IntervalMinutes+"분\n"+string.Join("\n",settings().AutoKnowledge.Rooms.Select(r=>"지식: "+r.Title))+"\n"+string.Join("\n",settings().SummaryForwards.Select(r=>r.Id+" · "+r.Label));
                        else if(command.Action=="help")reply=RelayCommandParser.Help;
                        else if(command.Action=="cancel") {progress.Pending=null;reply="대기 중인 변경을 취소했습니다.";}
                        else
                        {
                            var confirmed=false;
                            if(command.Action=="confirm")
                            {
                                var pending=progress.Pending; progress.Pending=null;store.Write(name,progress);
                                if(pending is null||pending.Code!=command.Value||pending.Expires<DateTimeOffset.UtcNow||pending.Revision!=CommandPolicy.Revision(settings()))throw new ArgumentException("확인번호가 없거나 만료됐거나 설정이 바뀌었습니다. 다시 요청하세요.");
                                command=pending.Command;confirmed=true;
                            }
                            var available=await ai.Chats.RoomsAsync(ct);
                            if(available.SingleOrDefault(r=>r.Profile==room.Profile&&r.Id==room.Id)?.DirectMemberId!=config.OperatorId)
                                throw new InvalidOperationException("제어방 상대방 검증이 변경되어 실행하지 않았습니다.");
                            var plan=CommandPolicy.Prepare(command,settings(),available,config);
                            if(!slash&&!confirmed)
                            {
                                var code=Convert.ToHexString(System.Security.Cryptography.RandomNumberGenerator.GetBytes(3));
                                var pinned=plan.Room is null?command:command with {Target=AiSettings.RoomKey(plan.Room.Profile,plan.Room.Id)};
                                progress.Pending=new(code,pinned,plan.Revision,DateTimeOffset.UtcNow.AddMinutes(5));
                                reply=plan.Description+"\n적용하려면 /확인 "+code+" (5분 이내). /취소";
                            }
                            else
                            {
                                progress.Pending=null;store.Write(name,progress);
                                before=CommandPolicy.Snapshot(settings());
                                reply=await execute(plan,ct);after=CommandPolicy.Snapshot(settings());
                            }
                        }
                    }
                }
                catch(OperationCanceledException) when(ct.IsCancellationRequested){throw;}
                catch(Exception e) {reply="실행하지 못했습니다: "+e.Message;}
                progress.History[^1]=new(message.Id,command?.Action??"parse",reply,DateTimeOffset.UtcNow,before,after);store.Write(name,progress);
                if (conversationHandled) { Report(reply); continue; }
                // Reply only to the authenticated room; fresh identity and unique title check precede sending.
                var fresh=await ai.Chats.ReadAsync(room.Profile,room.Id,1,ct);
                var all=await ai.Chats.RoomsAsync(ct);
                if(store.Load().Generation!=config.Generation||!store.Load().Enabled)return;
                if(!await ai.Chats.IsDirectPeerAsync(room,config.OperatorId,ct)||all.Count(r=>r.Title==fresh.Room.Title)!=1)throw new InvalidOperationException("제어 결과 수신방을 확정하지 못했습니다. 이력에서 확인하세요.");
                var requestId="command-"+AiConversation.Hash(config.Generation+":"+message.Id)[..40];
                try
                {
                    var receipt=await send(new(requestId,fresh.Room.Title,reply),ct);
                    if(!receipt.EnterPosted||receipt.InputCleared!=true)throw new IOException("회신 발송 확인 불가");
                    Report(reply);
                }
                catch(OperationCanceledException) when(ct.IsCancellationRequested){throw;}
                catch(Exception) {progress.History[^1]=progress.History[^1] with {Result=reply+" [회신 불확실 · 자동 재발송 안 함]"};store.Write(name,progress);Report("명령 처리 결과는 이력에 저장됐지만 회신을 확인하지 못했습니다.");}
            }
        }
        catch(OperationCanceledException) when(ct.IsCancellationRequested){throw;}
        catch(Exception e){Report(e.Message);}
        finally{Busy=false;gate.Release();}
    }
}
