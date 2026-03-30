// Generic stuff
////////////////////////////////////////////////////////////////////////////////

---types---

int ? = Int;
long ? = Long;
double ? = Double;
string ? = String;
object ? = Object;
function ? = Function;
bytes data:string = Bytes;
true = True;
boolTrue = Bool;
boolFalse = Bool;

vector {t:Type} # [ t ] = Vector t;

int128 4*[ int ] = Int128;
int256 8*[ int ] = Int256;


// RPC
////////////////////////////////////////////////////////////////////////////////

---types---

tonNode.preparedState = tonNode.PreparedState;
tonNode.notFoundState = tonNode.PreparedState;

tonNode.data data:bytes = tonNode.Data;

---functions---

tonNode.preparePersistentState block:tonNode.blockIdExt masterchain_block:tonNode.blockIdExt = tonNode.PreparedState;
tonNode.downloadPersistentStateSlice block:tonNode.blockIdExt masterchain_block:tonNode.blockIdExt offset:long max_size:long = tonNode.Data;
