
using breez.breez_sdk;

try
{
 var seed = BreezSdkMethods.MnemonicToSeed("cruise clever syrup coil cute execute laundry general cover prevent law sheriff");
 BreezSdkMethods.SetLogStream(new LogStreamListener());
 var config = BreezSdkMethods.DefaultConfig(EnvironmentType.STAGING, "code", new NodeConfig.Greenlight(new GreenlightNodeConfig(null, "")));
 BlockingBreezServices sdkServices = BreezSdkMethods.Connect(config, seed, new SDKListener());
 NodeState? nodeInfo = sdkServices.NodeInfo();
 Console.WriteLine(nodeInfo!.id);
}
catch (Exception e)
{
 Console.WriteLine(e.Message);
 throw;
}

class SDKListener : EventListener
{
 public void OnEvent(BreezEvent e)
 {
  Console.WriteLine("received event " + e);
 }
}

class LogStreamListener : LogStream
{
 public void Log(LogEntry l)
 {
  if (l.level != "TRACE") {
   Console.WriteLine(l.line);
  }
 }
}
