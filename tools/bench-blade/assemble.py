S=__import__('os').environ.get('BENCH_DIR', '.') + ''
import re
old=open(S+'/audio-limits.html').read()
css=old[old.index('<style>'):old.index('</style>')+8]
script=old[old.index('<script>'):old.index('</script>')+9]
def rel(own, off):
    return ' '.join(f"{f}:{float(d)+off:.1f}" for f,d in (x.split(':') for x in own.split()))
eq = "50:-1.1 63:-0.6 80:-0.3 100:-0.2 125:-0.2 160:-0.3 200:-0.3 250:-0.4 315:-0.4 400:-0.3 500:-0.3 630:-0.2 800:-0.1 1000:0.0 1250:0.3 1600:0.4 2000:0.0 2500:-0.4 3150:-0.6 4000:-0.3 5000:0.2 6300:-10.7 8000:-61.1"
eq8 = rel("50:-0.9 63:-0.4 80:-0.1 100:-0.1 125:-0.1 160:-0.2 200:-0.3 250:-0.4 315:-0.3 400:-0.3 500:-0.2 630:-0.1 800:-0.1 1000:0.0 1250:0.3 1600:0.4 2000:0.0 2500:-0.3 3150:-0.5 4000:-0.3 5000:0.3 6300:0.9 8000:-0.5", 1.1)
C = [
 ("idealtx","Transmit","Ideal: flat","--c0","2",True,"50:0 8000:0","6 4",False,True),
 ("txnow","Transmit","As set today (data set-up)","--c1","2.2",True,"50:-12.3 63:-10.1 80:-8.0 100:-6.2 125:-4.7 160:-3.5 200:-2.7 250:-2.1 315:-1.6 400:-1.1 500:-0.8 630:-0.5 800:-0.3 1000:0 1250:0.3 1600:0.4 2000:0.1 2500:0.7 3150:-7.4 4000:-56.0 5000:-68.0 6300:-68.0 8000:-64.5",None,False,False),
 ("txbyp","Transmit","Filter bypass alone (REG_47<0>)","--c3","1.8",False,"50:14.5 63:14.6 80:14.6 100:14.4 125:13.9 160:12.9 200:11.8 250:10.5 315:8.7 400:7.0 500:5.4 630:3.7 800:1.8 1000:0 1250:-1.9 1600:-4.0 2000:-6.1 2500:-8.2 3150:-10.6 4000:-13.4 5000:-16.6 6300:-21.4 8000:-30.2","5 3",False,False),
 ("txeq","Transmit","Bypass + equaliser eq (to 5 kHz)","--c5","2.6",True,eq,None,True,False),
 ("txeq8","Transmit","Bypass + equaliser eq8 (to 8 kHz, level 29)","--c6","2.6",True,eq8,None,True,False),
 ("idealrx","Receive","Ideal: flat","--c0","2",True,"50:0 8000:0","6 4",False,True),
 ("rxnow","Receive","As set today (REG_2B 0x0000)","--c5","2.2",True,"50:-58.8 63:-60.4 80:-56.7 100:-50.0 125:-57.9 160:-55.6 200:-54.8 250:-48.4 315:0.7 400:1.3 500:1.0 630:0.6 800:0.3 1000:0 1250:-1.6 1600:-3.8 2000:-6.3 2500:-8.5 3150:-20.7 4000:-37.5 5000:-45.1 6300:-64.7",None,False,False),
 ("rxhpf","Receive","300 Hz high-pass off (0x0400)","--c6","1.6",False,"50:-31.9 63:-25.2 80:-19.3 100:-14.2 125:-9.6 160:-5.5 200:-2.5 250:-0.2 315:1.2 400:1.8 500:1.6 630:1.2 800:0.6 1000:0 1250:-1.9 1600:-4.1 2000:-6.6 2500:-8.8 3150:-20.9 4000:-35.0 5000:-41.8 6300:-58.5","5 3",False,False),
 ("rxde","Receive","De-emphasis off (0x0100)","--c7","1.6",False,"50:-55.8 63:-55.6 80:-55.0 100:-54.4 125:-55.0 160:-53.6 200:-52.8 250:-51.0 315:-8.6 400:-6.2 500:-4.4 630:-2.9 800:-1.2 1000:0 1250:0.3 1600:0.3 2000:0.1 2500:-0.2 3150:-8.7 4000:-32.5 5000:-37.1 6300:-71.3","2 3",False,False),
 ("rxall","Receive","All three off (0x0700)","--c8","1.8",True,"50:-41.1 63:-34.8 80:-28.8 100:-23.5 125:-18.9 160:-14.6 200:-11.5 250:-8.8 315:-6.6 400:-4.5 500:-3.0 630:-1.6 800:-0.6 1000:0 1250:0.1 1600:0.0 2000:-0.5 2500:-1.0 3150:-10.2 4000:-33.3 5000:-36.5 6300:-67.4",None,False,False),
 ("rx9","Receive","Route 9 bypass + all off (0x9700)","--c4","2.8",True,"50:-40.7 63:-34.6 80:-28.5 100:-23.3 125:-18.7 160:-14.3 200:-11.2 250:-8.6 315:-6.4 400:-4.4 500:-2.8 630:-1.5 800:-0.5 1000:0 1250:0.2 1600:0.1 2000:-0.2 2500:-1.0 3150:-2.4 4000:-5.4 5000:-10.9 6300:-23.7",None,True,False),
]
def js(c):
    id_,g,n,col,w,on,pts,dash,best,ideal=c
    parts=[f"id: '{id_}'", f"group: '{g}'", f"name: '{n}'", f"c: '{col}'", f"w: {w}", f"on: {'true' if on else 'false'}"]
    if dash: parts.append(f"dash: '{dash}'")
    if best: parts.append("best: true")
    if ideal: parts.append("ideal: true")
    return "    { " + ", ".join(parts) + f",\n      pts: P('{pts}') }},"
series="  const S = [\n" + "\n".join(js(c) for c in C) + "\n  ];"
i=script.index('  const S = ['); j=script.index('  ];', i)+4
script=script[:i]+series+script[j:]
script=re.sub(r"audio-limits-picks\d*", "audio-limits-picks5", script)
body=open(S+'/clean_body.html').read()
head='<title>RT-950 Pro Audio Limits</title>\n<meta name="claude:viewport-fit-sides" content="cover">\n<link rel="preconnect" href="https://fonts.googleapis.com">\n<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>\n<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Barlow+Semi+Condensed:wght@500;600&family=Barlow:wght@400;500;600&family=JetBrains+Mono:wght@400;600&display=swap">\n'
open(S+'/audio-limits.html','w').write(head+css+"\n\n"+body+"\n\n"+script+"\n")
print('assembled')
