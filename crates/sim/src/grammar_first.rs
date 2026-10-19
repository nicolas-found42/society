//! First-clause rules keyed (cell, register). See `grammar.rs` for the slot
//! vocabulary and the coverage floor.
pub(crate) static FIRST: &[((&str, &str), &[&str])] = &[
    (
        ("take", "cold"),
        &[
            "I take {count} and answer nothing",
            "I take my share of grain from {store}",
        ],
    ),
    (
        ("take", "wary"),
        &[
            "I take {count} with steady hands",
            "I take little lest {other} mark me",
        ],
    ),
    (
        ("take", "plain"),
        &[
            "I take {count} from the store",
            "I take grain for my hearth at {place}",
        ],
    ),
    (
        ("take", "warm"),
        &[
            "I take {count} with a glad heart",
            "I take grain and thank the store at {place}",
        ],
    ),
    (
        ("take", "urgent"),
        &["I take {count} and run", "I take grain from {store} fast"],
    ),
    (
        ("give", "cold"),
        &[
            "I give grain to {person}, take it",
            "I give at {place} and keep no debt",
        ],
    ),
    (
        ("give", "wary"),
        &[
            "I give {person} grain though I fear want",
            "I give little for my store is thin",
        ],
    ),
    (
        ("give", "plain"),
        &[
            "I give {person} grain from my hand",
            "I give freely of my own store",
        ],
    ),
    (
        ("give", "warm"),
        &[
            "I give {person} grain with all my heart",
            "I give gladly and I wish you well",
        ],
    ),
    (
        ("give", "urgent"),
        &[
            "I give {person} grain, take it now",
            "I give fast for hunger presses hard",
        ],
    ),
    (
        ("forage", "cold"),
        &[
            "I seek roots beyond {place} alone",
            "I forage near {place} and tell no one",
        ],
    ),
    (
        ("forage", "wary"),
        &[
            "I pick through frost lest beasts stir near {place}",
            "I forage near {place} with soft tread",
        ],
    ),
    (
        ("forage", "plain"),
        &[
            "I forage for grain beyond {place}",
            "I gather wild seed with my hands at {place}",
        ],
    ),
    (
        ("forage", "warm"),
        &[
            "I forage gladly under bright sky at {place}",
            "I seek sweet roots for {person}",
        ],
    ),
    (
        ("forage", "urgent"),
        &[
            "I forage fast, the frost bites near",
            "I run to {place} for wild seed",
        ],
    ),
    (
        ("help", "cold"),
        &[
            "I lift your load a little way",
            "I lend my hands and nothing more at {place}",
        ],
    ),
    (
        ("help", "wary"),
        &[
            "I help {person} though I fear blame",
            "I lend my strength with careful eyes",
        ],
    ),
    (
        ("help", "plain"),
        &[
            "I help {person} bear the load",
            "I lend my hands to the work at {place}",
        ],
    ),
    (
        ("help", "warm"),
        &[
            "I help {person} with a willing heart",
            "I stand by {person} till work is done",
        ],
    ),
    (
        ("help", "urgent"),
        &[
            "I help {person} now, hold fast",
            "I run to lift with {person}",
        ],
    ),
    (
        ("steal", "cold"),
        &[
            "I take from {store} though none grant leave",
            "I lift grain and I hide my face at {place}",
        ],
    ),
    (
        ("steal", "wary"),
        &[
            "I steal softly lest {other} wake",
            "I lift a little and leave no mark at {place}",
        ],
    ),
    (
        ("steal", "plain"),
        &[
            "I steal grain from the store",
            "I take in secret from {store}",
        ],
    ),
    (
        ("steal", "warm"),
        &[
            "I steal for hunger, forgive my need",
            "I take and my heart is sore for {person}",
        ],
    ),
    (
        ("steal", "urgent"),
        &[
            "I steal and run through frost to {place}",
            "I snatch grain from {store} now",
        ],
    ),
    (
        ("accuse", "cold"),
        &[
            "{person}, I name thee thief before all",
            "I say {person} stole from {store}",
        ],
    ),
    (
        ("accuse", "wary"),
        &[
            "I fear {person} took from the store",
            "I deem {person} false but lack proof",
        ],
    ),
    (
        ("accuse", "plain"),
        &[
            "I hold {person} suspect of theft",
            "I charge {person} stole our grain from {store}",
        ],
    ),
    (
        ("accuse", "warm"),
        &[
            "I grieve to doubt good {person}",
            "I ask {person} to clear this doubt",
        ],
    ),
    (
        ("accuse", "urgent"),
        &[
            "I saw {person} take, stop them now",
            "{person}, I charge thee, halt at once",
        ],
    ),
    (
        ("promise", "cold"),
        &[
            "I pledge {obligation} and I keep it",
            "I swear I shall do this deed at {place}",
        ],
    ),
    (
        ("promise", "wary"),
        &[
            "I pledge {obligation} if frost spares me",
            "I promise though I fear I fail",
        ],
    ),
    (
        ("promise", "plain"),
        &[
            "I promise {obligation} before all",
            "I give my word to do this work",
        ],
    ),
    (
        ("promise", "warm"),
        &[
            "I promise {obligation} with glad heart",
            "I vow to keep my word to {person}",
        ],
    ),
    (
        ("promise", "urgent"),
        &[
            "I swear {obligation}, let me run",
            "I promise now by hearth and field",
        ],
    ),
    (
        ("draw_water", "cold"),
        &[
            "I draw {water} and waste no words",
            "I haul cold draughts from {place}",
        ],
    ),
    (
        ("draw_water", "wary"),
        &[
            "I draw {water} with careful tread",
            "I fetch water lest ice hold at {place}",
        ],
    ),
    (
        ("draw_water", "plain"),
        &["I draw {water} from the well", "I bear water from {place}"],
    ),
    (
        ("draw_water", "warm"),
        &[
            "I draw {water} with a merry heart",
            "I fetch clear water for {person}",
        ],
    ),
    (
        ("draw_water", "urgent"),
        &[
            "I run for {water}, the thirst bites",
            "I haul water to {place} fast",
        ],
    ),
    (
        ("chop", "cold"),
        &[
            "I fell and split the cold wood at {place}",
            "I chop {wood} and feed no talk",
        ],
    ),
    (
        ("chop", "wary"),
        &[
            "I chop {wood} with wary eyes",
            "I hew lest the axe slip wide",
        ],
    ),
    (
        ("chop", "plain"),
        &["I chop {wood} for the hearth", "I split logs by {place}"],
    ),
    (
        ("chop", "warm"),
        &[
            "I chop {wood} with a singing heart",
            "I hew good logs for {person}",
        ],
    ),
    (
        ("chop", "urgent"),
        &[
            "Wood, more wood, I haste to hew",
            "I chop {wood}, the frost is coming",
        ],
    ),
    (
        ("gather_herbs", "cold"),
        &[
            "I crop {herbs} and speak no word",
            "I gather bitter leaves alone at {place}",
        ],
    ),
    (
        ("gather_herbs", "wary"),
        &[
            "I seek {herbs} with wary tread",
            "I cull lest I take false leaf",
        ],
    ),
    (
        ("gather_herbs", "plain"),
        &[
            "I gather {herbs} by {place}",
            "I pick healing leaves with care",
        ],
    ),
    (
        ("gather_herbs", "warm"),
        &[
            "I gather {herbs} for dear {person}",
            "I seek herbs with a glad heart",
        ],
    ),
    (
        ("gather_herbs", "urgent"),
        &[
            "I seek {herbs}, haste with me",
            "I run for {herbs} ere light fails",
        ],
    ),
    (
        ("craft", "cold"),
        &[
            "I shape {tools} and brook no talk",
            "I craft in silence by my hearth at {place}",
        ],
    ),
    (
        ("craft", "wary"),
        &[
            "I mend {tools} lest they break at need",
            "I shape slow lest I spoil the work",
        ],
    ),
    (
        ("craft", "plain"),
        &[
            "I craft {tools} with my hands",
            "I shape wood into a sound tool",
        ],
    ),
    (
        ("craft", "warm"),
        &[
            "I craft {tools} with love and care",
            "I shape a fine tool for {person}",
        ],
    ),
    (
        ("craft", "urgent"),
        &[
            "I shape {tools} fast, bring wood",
            "I craft apace, the need is sore at {place}",
        ],
    ),
    (
        ("tend", "cold"),
        &[
            "I tend the field and spare no sweat",
            "I weed the rows in cold hush at {place}",
        ],
    ),
    (
        ("tend", "wary"),
        &[
            "I tend lest frost take the shoots at {place}",
            "I watch the field with wary eyes",
        ],
    ),
    (
        ("tend", "plain"),
        &[
            "I tend the field by {place}",
            "I weed and water with my hands",
        ],
    ),
    (
        ("tend", "warm"),
        &[
            "I tend glad shoots for us all at {place}",
            "I sing whilst I weed the rows",
        ],
    ),
    (
        ("tend", "urgent"),
        &[
            "I tend the field, come haste with me",
            "I run to save the tender shoots at {place}",
        ],
    ),
    (
        ("heal", "cold"),
        &[
            "I bind the wound and bid thee rest",
            "I lay my hands and staunch the blood",
        ],
    ),
    (
        ("heal", "wary"),
        &[
            "I heal {person} though I fear the ill",
            "I touch gently lest pain wake anew",
        ],
    ),
    (
        ("heal", "plain"),
        &[
            "I heal {person} with herbs and care",
            "I dress the wound with clean hands for {person}",
        ],
    ),
    (
        ("heal", "warm"),
        &[
            "I heal {person} with a tender heart",
            "I hold {person} till the fever breaks",
        ],
    ),
    (
        ("heal", "urgent"),
        &[
            "I heal {person}, fetch {herbs} fast",
            "I press hard, I staunch the flow",
        ],
    ),
    (
        ("trade", "cold"),
        &[
            "I trade my {coin} for needful grain",
            "I barter hard and yield no more at {place}",
        ],
    ),
    (
        ("trade", "wary"),
        &[
            "I trade wary lest I be cheated",
            "I weigh {coin} ere I strike hands",
        ],
    ),
    (
        ("trade", "plain"),
        &[
            "I trade {coin} at the market",
            "I swap my wares and hide my coin",
        ],
    ),
    (
        ("trade", "warm"),
        &[
            "I trade gladly and deal fair with {person}",
            "I offer my coin with an open heart",
        ],
    ),
    (
        ("trade", "urgent"),
        &[
            "I trade fast, take my {coin} now",
            "I strike hands ere the market breaks at {place}",
        ],
    ),
    (
        ("pray", "cold"),
        &[
            "I kneel to stone and wait no sign",
            "I pray though heaven keeps hush",
        ],
    ),
    (
        ("pray", "wary"),
        &[
            "I pray low lest spirits mark me",
            "I whisper prayers against the dark at {place}",
        ],
    ),
    (
        ("pray", "plain"),
        &[
            "I pray at the shrine today",
            "I ask blessing for my kin at {place}",
        ],
    ),
    (
        ("pray", "warm"),
        &[
            "I pray with thankful heart for all",
            "I bless this hearth with glad song",
        ],
    ),
    (
        ("pray", "urgent"),
        &[
            "I pray, haste, the night draws near",
            "I cry for aid ere all fails",
        ],
    ),
    (
        ("repair", "cold"),
        &[
            "I mend the breach and waste no breath",
            "I fix {place} with iron hands",
        ],
    ),
    (
        ("repair", "wary"),
        &[
            "I patch lest the wall fall at night",
            "I mend slow lest the beam slip",
        ],
    ),
    (
        ("repair", "plain"),
        &[
            "I repair the roof with my hands",
            "I mend {place} ere rains come",
        ],
    ),
    (
        ("repair", "warm"),
        &[
            "I mend with care for {person}",
            "I make {place} sound for us all",
        ],
    ),
    (
        ("repair", "urgent"),
        &[
            "I mend fast ere the roof falls",
            "I haste to brace the sagging wall at {place}",
        ],
    ),
    (
        ("console", "cold"),
        &[
            "I sit by thee, be still now",
            "I hear thy grief and say little",
        ],
    ),
    (
        ("console", "wary"),
        &[
            "I grieve with {person} soft and slow",
            "I speak low lest I wound anew",
        ],
    ),
    (
        ("console", "plain"),
        &[
            "I comfort {person} in their grief",
            "I sit with {person} through the night",
        ],
    ),
    (
        ("console", "warm"),
        &[
            "I hold {person} close with tender heart",
            "I weep with {person} and give comfort",
        ],
    ),
    (
        ("console", "urgent"),
        &[
            "I come to {person}, hold fast now",
            "I haste to soothe thy fearful heart",
        ],
    ),
    (
        ("rest", "cold"),
        &[
            "I rest my bones and want silence",
            "I lie still and shut out talk at {place}",
        ],
    ),
    (
        ("rest", "wary"),
        &[
            "I rest light lest danger stir",
            "I doze with one eye on {place}",
        ],
    ),
    (
        ("rest", "plain"),
        &[
            "I rest my limbs beside the hearth",
            "I sleep till my strength returns",
        ],
    ),
    (
        ("rest", "warm"),
        &[
            "I rest glad in good company at {place}",
            "I dream soft by the warm hearth",
        ],
    ),
    (
        ("rest", "urgent"),
        &[
            "I rest a breath then rise again",
            "I pause though haste still drives me on",
        ],
    ),
    (
        ("go", "cold"),
        &["I go to {place} and tarry not", "I walk alone to {place}"],
    ),
    (
        ("go", "wary"),
        &[
            "I slip to {place} by hidden ways",
            "I go wary lest eyes track me",
        ],
    ),
    (
        ("go", "plain"),
        &["I go to {place} today", "I walk to {place} with my kin"],
    ),
    (
        ("go", "warm"),
        &[
            "I walk glad to {place} with {person}",
            "I go to {place} with lightsome heart",
        ],
    ),
    (
        ("go", "urgent"),
        &[
            "I run to {place}, follow fast",
            "I haste to {place}, come with me",
        ],
    ),
    (
        ("act:plea", "cold"),
        &[
            "I beg bread though pride burns sore",
            "I ask, I need, deny me not",
        ],
    ),
    (
        ("act:plea", "wary"),
        &[
            "I plead low lest scorn fall on me",
            "I beg {person} spare a little grain",
        ],
    ),
    (
        ("act:plea", "plain"),
        &[
            "I plead for bread for my kin",
            "I ask mercy of {person} today",
        ],
    ),
    (
        ("act:plea", "warm"),
        &[
            "I pray thee help me in my need",
            "I beg with thankful tears, good {person}",
        ],
    ),
    (
        ("act:plea", "urgent"),
        &[
            "I beg, I hunger, help me now",
            "I plead ere my strength quite fails",
        ],
    ),
    (
        ("act:gratitude", "cold"),
        &[
            "I thank thee, debt is paid now",
            "I owe thee thanks, I own it",
        ],
    ),
    (
        ("act:gratitude", "wary"),
        &[
            "I thank {person} though words come hard",
            "I hold thy kindness close in heart",
        ],
    ),
    (
        ("act:gratitude", "plain"),
        &[
            "I thank {person} for this gift",
            "I am glad for thy good help",
        ],
    ),
    (
        ("act:gratitude", "warm"),
        &[
            "I thank {person} with all my heart",
            "I bless the day you came here",
        ],
    ),
    (
        ("act:gratitude", "urgent"),
        &[
            "I thank thee fast, I must run",
            "I bless {person}, I haste away",
        ],
    ),
    (
        ("act:gossip", "cold"),
        &[
            "{other} keeps secrets, I mark it well",
            "I saw {other} creep by {place}",
        ],
    ),
    (
        ("act:gossip", "wary"),
        &[
            "I hear {other} meets {person} at dusk",
            "They say {other} hides grain, I fear",
        ],
    ),
    (
        ("act:gossip", "plain"),
        &[
            "I hear {other} wed ere harvest",
            "I saw {other} talk long with {person}",
        ],
    ),
    (
        ("act:gossip", "warm"),
        &[
            "I laugh to tell of dear {other}",
            "I bring glad tales of {other} today",
        ],
    ),
    (
        ("act:gossip", "urgent"),
        &[
            "I heard of {other}, hark and run",
            "{other} flees to {place}, I swear",
        ],
    ),
    (
        ("act:forgiveness", "cold"),
        &[
            "I forgive {person} and close the count",
            "I pardon, but I forget nothing",
        ],
    ),
    (
        ("act:forgiveness", "wary"),
        &[
            "I forgive slow though my heart still aches",
            "I loose my grudge lest hate fester",
        ],
    ),
    (
        ("act:forgiveness", "plain"),
        &[
            "I forgive {person} before all",
            "I let old wrongs lie still",
        ],
    ),
    (
        ("act:forgiveness", "warm"),
        &[
            "I forgive {person} with open heart",
            "I clasp {person} and call us kin",
        ],
    ),
    (
        ("act:forgiveness", "urgent"),
        &[
            "I forgive now, haste to mend with {person}",
            "I pardon {person}, let us run",
        ],
    ),
    (
        ("act:blessing", "cold"),
        &[
            "I wish thee hardness for the road",
            "I bless with stone, endure and stand",
        ],
    ),
    (
        ("act:blessing", "wary"),
        &[
            "I bless lest curse else light here",
            "I wish {person} shelter from the storm",
        ],
    ),
    (
        ("act:blessing", "plain"),
        &[
            "I bless this house and hearth",
            "I wish {person} health and grain",
        ],
    ),
    (
        ("act:blessing", "warm"),
        &[
            "I bless {person} with joy and grain",
            "I wish thee sunshine after rain",
        ],
    ),
    (
        ("act:blessing", "urgent"),
        &[
            "I bless thee fast, now run",
            "I bless {person}, haste and live",
        ],
    ),
    (
        ("act:threat", "cold"),
        &[
            "{person}, I shall cast thee out",
            "I warn {person}, cross me no more",
        ],
    ),
    (
        ("act:threat", "wary"),
        &[
            "I warn {person} lest blood be spilt",
            "{person}, I am watching, tread with care",
        ],
    ),
    (
        ("act:threat", "plain"),
        &[
            "I shall drive {person} from this {place}",
            "{person}, I will meet thee staff to staff",
        ],
    ),
    (
        ("act:threat", "warm"),
        &[
            "I warn {person} though I love thee still",
            "{person}, I pray thee yield ere I strike",
        ],
    ),
    (
        ("act:threat", "urgent"),
        &[
            "{person}, I come, stand and fight",
            "I strike now, {person}, guard thyself",
        ],
    ),
    (
        ("act:counsel", "cold"),
        &[
            "I rede thee keep thy grain close",
            "I counsel silence, hold thy tongue",
        ],
    ),
    (
        ("act:counsel", "wary"),
        &[
            "I counsel care ere thou trust {other}",
            "I deem it wise to watch and wait",
        ],
    ),
    (
        ("act:counsel", "plain"),
        &[
            "I counsel patience till the thaw",
            "I rede {person} mend {place} first",
        ],
    ),
    (
        ("act:counsel", "warm"),
        &[
            "I counsel hope, good {person}, hold fast",
            "I guide with love, keep {obligation}",
        ],
    ),
    (
        ("act:counsel", "urgent"),
        &[
            "I warn thee, haste ere frost bites",
            "I counsel flight to {place} now",
        ],
    ),
    (
        ("act:none", "cold"),
        &[
            "{person}, I bid thee say what you mean",
            "I have no words to waste here",
        ],
    ),
    (
        ("act:none", "wary"),
        &[
            "I keep my thoughts close today",
            "I watch and say but little",
        ],
    ),
    (
        ("act:none", "plain"),
        &[
            "I walk these lanes most days",
            "I know the paths by {place}",
        ],
    ),
    (
        ("act:none", "warm"),
        &[
            "I am glad to sit with {person}",
            "I love tales by the warm hearth",
        ],
    ),
    (
        ("act:none", "urgent"),
        &[
            "I haste, I cannot tarry here",
            "I run, the hour presses hard",
        ],
    ),
];
