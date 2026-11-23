using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using KakaoRelay.Core;
namespace KakaoRelay.App;
public partial class CommandRoomPanel:UserControl,INotifyPropertyChanged
{
    private readonly CommandRoomStore store=new();
    private CommandRoomService? commands;
    private AiService? ai;
    private Func<LocalRoom,Task>? reserve;
    private readonly System.Windows.Threading.DispatcherTimer timer=new(){Interval=TimeSpan.FromSeconds(2)};
    private CancellationTokenSource? tickCancellation;
    private Task running=Task.CompletedTask;
    private bool editing,stopping;
    public CommandRoomSettings Draft {get;private set;}=new();
    public ObservableCollection<LocalRoom> Rooms {get;}=[];
    private LocalRoom? selected;
    public LocalRoom? SelectedRoom {get=>selected;set{selected=value;Changed();}}
    public string Operator=>SelectedRoom?.DirectMemberId is {} id?"허용 상대 ID: "+id:"일반 1:1 방을 선택하세요.";
    public string Status {get;private set;}="제어방 꺼짐";
    public string History=>commands is null?"":string.Join("\n\n",commands.History().AsEnumerable().Reverse().Select(a=>$"{a.At.ToLocalTime():MM/dd HH:mm:ss} · {a.Action}\n{a.Result}"));
    public string Help=>RelayCommandParser.Help;
    public object RuntimeStatus => new { enabled=store.Load().Enabled, busy=commands?.Busy??false, status=commands?.Status??Status };
    public event PropertyChangedEventHandler? PropertyChanged;
    private void Changed()=>PropertyChanged?.Invoke(this,new(string.Empty));
    public CommandRoomPanel(){InitializeComponent();DataContext=this;timer.Tick+=(_,_)=>StartTick();}
    public bool Owns(LocalRoom room)=>store.Load().Owns(room);
    public void Initialize(AiService service,Func<AiSettings> settings,Func<CommandPlan,CancellationToken,Task<string>> execute,Func<string> status,Func<ApiSendCommand,CancellationToken,Task<TestSendReceipt>> send,Func<LocalRoom,Task> reserveRoom)
    {
        ai=service;reserve=reserveRoom;Draft=store.Load();
        if(Draft.Room is {} room){Rooms.Add(room);SelectedRoom=room;}
        commands=new(service,store,settings,execute,status,send);
        commands.StatusChanged+=s=>Dispatcher.Invoke(()=>{Status=s;Changed();});
        Status=Draft.Enabled?"제어방 재개 중":"제어방 꺼짐";timer.Start();StartTick();Changed();
    }
    private void StartTick(){if(stopping||editing||!running.IsCompleted||commands is null)return;running=Tick();}
    private async Task Tick(){using var ct=new CancellationTokenSource();tickCancellation=ct;try{await commands!.TickAsync(ct.Token);}catch(OperationCanceledException){}finally{tickCancellation=null;Changed();}}
    public void Stop(){stopping=true;timer.Stop();tickCancellation?.Cancel();commands?.Stop();}
    public async Task DrainAsync(){await running;if(commands is not null)await commands.DrainAsync();}
    private async void Refresh_Click(object sender,RoutedEventArgs e)
    {
        if(ai is null||editing)return;editing=true;
        try{var previous=SelectedRoom;var rooms=await ai.Chats.RoomsAsync();Rooms.Clear();foreach(var r in rooms.Where(r=>r.DirectMemberId is not null))Rooms.Add(r);SelectedRoom=Rooms.FirstOrDefault(r=>r.Profile==previous?.Profile&&r.Id==previous.Id);}
        catch(Exception error){Status=error.Message;}
        finally{editing=false;Changed();}
    }
    private async void Apply_Click(object sender,RoutedEventArgs e)
    {
        if(ai is null||editing)return;editing=true;tickCancellation?.Cancel();
        try
        {
            await running;
            if(Draft.Enabled)
            {
                if(SelectedRoom is null)throw new InvalidOperationException("1:1 제어방을 선택하세요.");
                var ready=await AutoReplyReadiness.PrepareAsync(ai.Chats,[SelectedRoom],ConversationCatalog.Scan);
                SelectedRoom=ready.Selected.Single();
                if(SelectedRoom.DirectMemberId is null||SelectedRoom.DirectMemberId==ai.Settings.Load().SelfId)throw new InvalidOperationException("상대방이 있는 일반 1:1 방만 허용합니다.");
                await reserve!(SelectedRoom);
            }
            Draft.Room=SelectedRoom;Draft.OperatorId=SelectedRoom?.DirectMemberId??"";Draft.Generation=Guid.NewGuid();store.Save(Draft);
            Status=Draft.Enabled?"제어방 적용 · 새 명령부터 처리":"제어방 꺼짐";
        }
        catch(Exception error){Status=error.Message;}
        finally{editing=false;Changed();StartTick();}
    }
}
