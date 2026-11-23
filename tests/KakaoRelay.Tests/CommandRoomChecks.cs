using System.Text.Json;
using KakaoRelay.Core;
internal static class CommandRoomChecks
{
    private sealed class Reader:IChatReader
    {
        public List<LocalRoom> Rooms=[new("p","1","관리",true){DirectMemberId="2"},new("p","2","작업",true),new("p","3","다른방",true)];
        public List<LocalMessage> Messages=[];
        public Task<List<LocalRoom>> RoomsAsync(CancellationToken ct=default)=>Task.FromResult(Rooms.ToList());
        public Task<ChatContext> ReadAsync(string p,string id,int limit,CancellationToken ct=default)=>Task.FromResult(new ChatContext(Rooms.Single(r=>r.Id==id),Messages.TakeLast(limit).ToList(),DateTimeOffset.UtcNow));
    }
    private sealed class Runner:IAiRunner
    {
        public RelayCommand Command=new("none");public int Calls;public int Chats;public string LastChat="";public string Reply="안녕하세요. 이야기해 주세요.";
        public Task<string> RunAsync(AiProviderSettings p,string prompt,int timeout,CancellationToken ct){Calls++;if(!prompt.Contains("릴레이 관리 명령을 JSON 하나로 분류한다")){Chats++;LastChat=prompt;return Task.FromResult(Reply);}return Task.FromResult(JsonSerializer.Serialize(Command,CommandRoomStore.Json));}
    }
    private sealed class Images(string path) : IChatImageGenerator
    {
        public int Calls;
        public Task<string> GenerateAsync(AiSettings settings,string provider,string prompt,CancellationToken ct,IProgress<string>? progress)
        { Calls++; return Task.FromResult(path); }
    }
    public static async Task RunAsync(string root,Action<bool,string> check)
    {
        var reader=new Reader();var runner=new Runner();var settingsStore=new AiSettingsStore(Path.Combine(root,"command-settings.json"));var settings=settingsStore.Load();settings.SelfId="1";settingsStore.Save(settings);
        var config=new CommandRoomSettings{Enabled=true,Room=reader.Rooms[0],OperatorId="2",NaturalLanguage=true};
        var store=new CommandRoomStore(Path.Combine(root,"commands"));store.Save(config);
        var images=new Images(Path.Combine(root,"generated.png"));var ai=new AiService(reader,settingsStore,runner,images);var sent=new List<ApiSendCommand>();var executed=0;bool failSend=false;
        CommandRoomService Service()=>new(ai,store,()=>settings,(plan,ct)=>{executed++;settings=plan.Next;return Task.FromResult("적용: "+plan.Description);},()=>"자동답장 0개",(command,ct)=>{sent.Add(command);return Task.FromResult(new TestSendReceipt{EnterPosted=true,InputCleared=!failSend});});
        var service=Service();int number=0;
        void Add(string text,string author="2",int type=1,bool deleted=false)=>reader.Messages.Add(new((++number).ToString(),author,author,DateTimeOffset.UtcNow,type,text,deleted));
        Add("/답변방식 즉시 작업");await service.TickAsync();check(executed==0&&sent.Count==0,"Command activation baselines old messages without replay");
        Add("/답변방식 즉시 작업","3");Add("/답변방식 즉시 작업",type:77);Add("/답변방식 즉시 작업",deleted:true);Add("/답변방식 즉시 작업","1");await service.TickAsync();check(executed==0&&sent.Count==0,"Unauthorized, self, unsupported and deleted messages cannot issue commands");
        Add("/답변방식 즉시 작업");await service.TickAsync();check(executed==1&&settings.ReplyModeForRoom("p","2")=="immediate","Explicit authenticated command applies validated setting");
        await Service().TickAsync();check(executed==1&&sent.Count==1,"Restart and repeated polling do not replay a command");
        runner.Command=new("mode","작업","호출");Add("작업은 부를 때만 답해");await service.TickAsync();check(executed==1&&sent[^1].Message.Contains("/확인"),"Natural language mutation previews instead of executing");
        var progress=store.Read<CommandRoomService.Progress>(config.Generation+".json")!;check(progress.Pending!.Command.Target==AiSettings.RoomKey("p","2"),"Confirmation pins a room identity rather than its mutable title");reader.Rooms[1]=reader.Rooms[1] with {Title="바뀐이름"};Add("/확인 "+progress.Pending!.Code);await service.TickAsync();check(executed==2&&settings.ReplyModeForRoom("p","2")=="trigger","Confirmation executes the exact prepared natural-language command");reader.Rooms[1]=reader.Rooms[1] with {Title="작업"};
        Add("작업을 바꿔");await service.TickAsync();progress=store.Read<CommandRoomService.Progress>(config.Generation+".json")!;settings.PollSeconds=7;Add("/확인 "+progress.Pending!.Code);await service.TickAsync();check(executed==2&&sent[^1].Message.Contains("설정이 바뀌"),"Changed settings invalidate a pending confirmation");
        Add("작업을 바꿔");await service.TickAsync();progress=store.Read<CommandRoomService.Progress>(config.Generation+".json")!;var code=progress.Pending!.Code;progress.Pending=progress.Pending with{Expires=DateTimeOffset.UtcNow.AddMinutes(-1)};store.Write(config.Generation+".json",progress);Add("/확인 "+code);await service.TickAsync();check(executed==2,"Expired confirmation cannot execute");
        config.Reply=false;store.Save(config);Add("/답변방식 맥락 작업");await service.TickAsync();check(executed==2&&sent[^1].Message.Contains("허용하지"),"App-only permission denies an otherwise valid command");config.Reply=true;store.Save(config);
        reader.Rooms.Add(new("p","4","작업",true));Add("/답변방식 맥락 작업");await service.TickAsync();check(executed==2&&sent[^1].Message.Contains("특정"),"Ambiguous room names never mutate settings");reader.Rooms.RemoveAt(3);
        reader.Rooms[0]=reader.Rooms[0] with{DirectMemberId=null};Add("/답변방식 맥락 작업");await service.TickAsync();check(executed==2&&service.Status.Contains("검증 실패"),"Changed room membership stops command execution");
        reader.Rooms[0]=config.Room!;await service.TickAsync();check(executed==3,"Authenticated room can resume processing after identity is restored");
        failSend=true;Add("/답변방식 즉시 작업");await service.TickAsync();var sends=sent.Count;await service.TickAsync();check(executed==4&&sent.Count==sends&&service.History()[^1].Result.Contains("회신 불확실"),"Uncertain reply is audited without replaying mutation or send");failSend=false;
        Add("/상태");await service.TickAsync();check(sent[^1].Message.Contains("자동답장 0개"),"Control status responds even when no auto-reply room runs");
        runner.Command=new("none"); var beforeChat=executed;
        Add("오늘 뭐 하고 있어?");await service.TickAsync();
        check(runner.Chats==1&&sent[^1].Recipient=="관리"&&sent[^1].Message.Contains("안녕하세요")&&executed==beforeChat,"Ordinary control-room messages receive a persona reply without executing settings");
        check(runner.LastChat.Contains(number.ToString())&&service.History()[^1].Action=="chat","Conversation targets the incoming message and is audited as chat");
        var chatSends=sent.Count;await Service().TickAsync();check(sent.Count==chatSends,"Restart does not replay a conversational reply");
        config.NaturalLanguage=false;store.Save(config);var calls=runner.Calls;
        Add("안녕");await service.TickAsync();check(runner.Chats==2&&runner.Calls==calls+1,"Disabling natural-language commands still permits ordinary conversation without classification");
        Add("/상태");await service.TickAsync();check(runner.Chats==2&&sent[^1].Message.Contains("자동답장 0개"),"Slash commands retain priority when natural-language commands are disabled");
        Add("/없는명령");await service.TickAsync();check(runner.Chats==2&&sent[^1].Message.Contains("명령 형식"),"Invalid slash commands do not fall through to ordinary conversation");
        config.NaturalLanguage=true;store.Save(config);runner.Command=new("mode","작업","호출");
        Add("작업 답변 방식 변경해");await service.TickAsync();var pendingChat=store.Read<CommandRoomService.Progress>(config.Generation+".json")!.Pending!;
        runner.Command=new("none");Add("잠깐 다른 이야기 하자");await service.TickAsync();
        check(store.Read<CommandRoomService.Progress>(config.Generation+".json")!.Pending?.Code==pendingChat.Code&&executed==beforeChat,"Ordinary conversation preserves a pending confirmation without applying it");
        runner.Command=new("none");runner.Reply="고양이를 그릴게요.\n[[generate_image:\"달 위의 고양이\"]]";
        var imageStart=sent.Count;Add("고양이 그려줘");await service.TickAsync();
        check(images.Calls==1&&sent.Count==imageStart+2&&!sent[imageStart].IsImage&&sent[imageStart+1].IsImage,"Control-room conversation uses normal introduction then generated-image delivery without an extra command reply");
        var imageSent=sent.Count;await Service().TickAsync();check(sent.Count==imageSent,"Generated image is not replayed by another poller instance");
        runner.Reply="첨부 확인했어요.";var chatCount=runner.Chats;
        Add("사진",type:2);await service.TickAsync();Add("자료.pdf",type:18);await service.TickAsync();
        check(runner.Chats==chatCount+2&&executed==beforeChat,"Photos and files enter normal conversation without becoming administrative commands");
        runner.Reply="고양이를 그릴게요.\n[[generate_image:\"고양이\"]]";failSend=true;var failedStart=sent.Count;var generatedBefore=images.Calls;
        Add("다시 그려줘");await service.TickAsync();await service.TickAsync();failSend=false;
        check(sent.Count==failedStart+1&&images.Calls==generatedBefore,"Uncertain introduction prevents image generation and duplicate fallback sends");
        service.Stop();await service.DrainAsync();service=Service();runner.Reply="안녕하세요. 이야기해 주세요.";
        config.Enabled=false;store.Save(config);Add("/답변방식 호출 작업");await service.TickAsync();check(executed==4,"Disabling command room stops all command processing");
        bool Reject(RelayCommand c){try{CommandPolicy.Prepare(c,settings,reader.Rooms,config);return false;}catch{return true;}}
        check(Reject(new("shell","","whoami"))&&Reject(new("knowledge-interval",Value:"0"))&&Reject(new("provider",Value:"unregistered")),"Unsupported operations, invalid intervals and providers are rejected");
        check(Reject(new("knowledge","작업","켜기")),"Knowledge activation requires a preconnected vault");
        config.Enabled=true;check(Reject(new("reply","관리","켜기")),"Control room cannot become an ordinary reply room");
        check(RelayCommandParser.Parse("/페르소나 업무 봇 | 작업")==new RelayCommand("persona","작업","업무 봇"),"Explicit persona syntax preserves names containing spaces");
        using var timeout=new CancellationTokenSource(TimeSpan.FromSeconds(10));
        settingsStore.Save(settings);
        var runtime=new ReplyRuntime(ai,(_,_)=>Task.FromResult(new TestSendReceipt()),()=>[new("작업",1,"0x1",false),new("다른방",1,"0x2",false)]);
        await runtime.StartAsync([reader.Rooms[1],reader.Rooms[2]],timeout.Token);check(runtime.Count==2,"Reply runtime starts independently managed rooms");
        await runtime.StopAsync(reader.Rooms[1]);check(runtime.Count==1&&runtime.Running(reader.Rooms[2]),"Stopping one room leaves the other room running");
        await runtime.StopAsync();check(runtime.Count==0,"Stopping all reply rooms drains their sessions");
        await using var api=await RelayApi.StartAsync(Path.Combine(root,"runtime-connection.json"),()=>[],()=>[],_=>Task.FromResult(new TestSendReceipt()),runtime:()=>Task.FromResult<object>(new {activeRooms=runtime.Count}));
        using var client=new HttpClient{BaseAddress=new Uri(api.Connection.BaseUrl)};
        check((await client.GetAsync("/v1/ai/runtime")).StatusCode==System.Net.HttpStatusCode.Unauthorized,"Runtime status requires gateway authentication");
        client.DefaultRequestHeaders.Authorization=new("Bearer",api.Connection.Token);
        var response=await client.GetStringAsync("/v1/ai/runtime");
        check(response.Contains("\"activeRooms\":0"),"Runtime API reports actual stopped rooms independently of API readiness");
    }
}
